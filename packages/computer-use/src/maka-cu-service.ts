// Supervises one `maka-cu` executor child and speaks `maka.cu/2` to it over
// line-delimited JSON-RPC 2.0 on stdio (`maka-cu`'s docs/HOST_PROTOCOL.md §1).
//
// The framing decoder and the lifecycle vocabulary are shared with the
// cua-driver service (stdio-json-rpc.ts). The supervision policy is not, and
// that is deliberate: this executor cancels with `$/cancel` and waits for its
// own answer instead of being killed (§7.2), shuts down on SIGTERM with a
// declared grace window (§11), owns its image directory (§8), and has one child
// rather than a role pair. Folding those into the cua-driver supervisor would
// mean a constructor flag per divergence, and every flag is a chance to run
// maka-cu's teardown against cua-driver.
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { AsyncLocalStorage } from 'node:async_hooks';
import { createHash } from 'node:crypto';
import { constants as fsConstants } from 'node:fs';
import { access, mkdir, readFile, realpath, rm } from 'node:fs/promises';
import {
  MAKA_CU_ALLOW_GLOBAL_POINTER,
  MAKA_CU_PROTOCOL_VERSION,
  MAKA_CU_RPC_ERROR,
  type MakaCuEnvelope,
  type MakaCuRpcErrorBody,
  type MakaCuRpcResponse,
  readEnvelope,
} from './maka-cu-protocol.js';
import {
  abortPromise,
  decodeJsonLines,
  type HostLifecycleErrorCode,
  type HostRequestStage,
} from './stdio-json-rpc.js';

const DEFAULT_HANDSHAKE_TIMEOUT_MS = 10_000;
const DEFAULT_REQUEST_TIMEOUT_MS = 20_000;
const DEFAULT_MAX_RESTART_ATTEMPTS = 3;
const DEFAULT_RESTART_BACKOFF_MS = 50;
/** No image ever crosses stdout (§8), so the only large payload left is the AX
 *  tree, itself bounded by `limits.maxResponseBytes`. Two of those plus slack. */
const MAX_STDOUT_BUFFER = 4 * 1024 * 1024;
const STDERR_TAIL_CAP = 4096;
/** §1: three non-JSON stdout lines in one generation are grounds for teardown. */
const MAX_NON_JSON_LINES = 3;
/** Used only until the handshake declares `limits.shutdownGraceMs` (§2). */
const FALLBACK_SHUTDOWN_GRACE_MS = 3_000;

export type MakaCuServiceState =
  | 'idle'
  | 'starting'
  | 'ready'
  | 'backing_off'
  | 'unavailable'
  | 'disposed';

export interface MakaCuServiceSnapshot {
  state: MakaCuServiceState;
  generation: number;
  restartAttempts: number;
  executor?: MakaCuExecutorInfo;
}

export interface MakaCuReleaseEvent {
  generation: number;
  generationReleased: boolean;
  reason:
    | 'child_exit'
    | 'request_timeout'
    | 'protocol_violation'
    | 'session_cleared'
    | 'restart_exhausted'
    | 'disposed';
  sessionIds: readonly string[];
  outcomeUnknown: boolean;
}

export class MakaCuLifecycleError extends Error {
  constructor(
    readonly code: HostLifecycleErrorCode,
    message: string,
    readonly generation: number,
    readonly requestStage?: HostRequestStage,
  ) {
    super(`${code}: ${message}`);
    this.name = 'MakaCuLifecycleError';
  }
}

export function isMakaCuLifecycleError(
  error: unknown,
  code?: HostLifecycleErrorCode,
): error is MakaCuLifecycleError {
  return error instanceof MakaCuLifecycleError && (code === undefined || error.code === code);
}

/** A JSON-RPC `error` (§1.1): the request was unusable, not a fact about the world. */
export class MakaCuRpcError extends Error {
  constructor(
    readonly method: string,
    readonly body: MakaCuRpcErrorBody,
  ) {
    super(`maka-cu ${method}: ${body.message} (${body.code})`);
    this.name = 'MakaCuRpcError';
  }
}

export interface MakaCuExecutorInfo {
  name: string;
  version: string;
  commit?: string;
}

export interface MakaCuCapabilities {
  captureStream: boolean;
  elementActions: readonly string[];
  pointActions: readonly string[];
  keyActions: readonly string[];
  imageFormats: readonly string[];
}

/** §2: every one of these has a host consumer; none may be hardcoded here. */
export interface MakaCuLimits {
  snapshotsPerSession: number;
  snapshotTtlMs: number;
  maxElements: number;
  maxDepth: number;
  maxTextChars: number;
  maxResponseBytes: number;
  settleCeilingMs: number;
  shutdownGraceMs: number;
  imageDirBudgetBytes: number;
}

export interface MakaCuHandshake {
  executor: MakaCuExecutorInfo;
  pid: number;
  capabilities: MakaCuCapabilities;
  limits: MakaCuLimits;
}

export interface MakaCuServiceOptions {
  /** Absolute path to the `maka-cu` executable; spawned as a DIRECT child (§11). */
  binaryPath: string;
  /** Host-owned image directory, purged before every spawn (§8, §11). */
  imageDir: string;
  hostVersion: string;
  timeoutMs?: number;
  handshakeTimeoutMs?: number;
  maxRestartAttempts?: number;
  restartBackoffMs?: number;
  childEnv?: NodeJS.ProcessEnv;
  expectedBinarySha256?: string;
  /** Test seam: the protocol string sent in `host.hello`. */
  protocolVersion?: string;
  onRelease?: (event: MakaCuReleaseEvent) => void;
}

interface PendingRequest {
  sessionId?: string;
  stage: HostRequestStage;
  resolve: (response: MakaCuRpcResponse) => void;
  reject: (error: Error) => void;
}

export class MakaCuService {
  private readonly childEnv: NodeJS.ProcessEnv;
  private child?: ChildProcessWithoutNullStreams;
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private buffer = '';
  private stderrTail = '';
  private nonJsonLines = 0;
  private starting?: Promise<void>;
  private disposed = false;
  private generation = 0;
  private state: MakaCuServiceState = 'idle';
  private restartAttempts = 0;
  private nextRestartAt?: number;
  private handshake?: MakaCuHandshake;
  private readonly sessionContext = new AsyncLocalStorage<string>();

  constructor(private readonly opts: MakaCuServiceOptions) {
    this.childEnv = { ...(opts.childEnv ?? process.env) };
  }

  snapshot(): MakaCuServiceSnapshot {
    return {
      state: this.state,
      generation: this.generation,
      restartAttempts: this.restartAttempts,
      ...(this.handshake ? { executor: this.handshake.executor } : {}),
    };
  }

  /** Handshake facts (§2). Undefined until the first successful start. */
  negotiated(): MakaCuHandshake | undefined {
    return this.handshake;
  }

  async withSession<T>(sessionId: string, operation: () => Promise<T>): Promise<T> {
    return this.sessionContext.run(sessionId, operation);
  }

  private assertActive(): void {
    if (this.disposed) {
      throw new MakaCuLifecycleError(
        'service_unavailable',
        'maka-cu service disposed',
        this.generation,
      );
    }
  }

  private emitRelease(
    reason: MakaCuReleaseEvent['reason'],
    sessionIds: readonly string[],
    outcomeUnknown: boolean,
    generationReleased: boolean,
  ): void {
    this.opts.onRelease?.({
      generation: this.generation,
      generationReleased,
      reason,
      sessionIds: [...new Set(sessionIds)],
      outcomeUnknown,
    });
  }

  async ensureStarted(signal?: AbortSignal): Promise<MakaCuHandshake> {
    this.assertActive();
    if (this.child && !this.child.killed && this.state === 'ready' && this.handshake) {
      return this.handshake;
    }
    if (!this.starting) {
      this.starting = this.startWithBudget().finally(() => {
        this.starting = undefined;
      });
    }
    const starting = this.starting;
    if (signal) await Promise.race([starting, abortPromise(signal)]);
    else await starting;
    if (!this.handshake) {
      throw new MakaCuLifecycleError(
        'service_unavailable',
        'maka-cu is not ready',
        this.generation,
      );
    }
    return this.handshake;
  }

  private async startWithBudget(): Promise<void> {
    const maxAttempts = this.opts.maxRestartAttempts ?? DEFAULT_MAX_RESTART_ATTEMPTS;
    let lastError: unknown;
    while (this.restartAttempts < maxAttempts) {
      this.assertActive();
      if (this.nextRestartAt !== undefined) {
        const delayMs = Math.max(0, this.nextRestartAt - Date.now());
        if (delayMs > 0) {
          this.state = 'backing_off';
          await new Promise((resolve) => setTimeout(resolve, delayMs));
          this.assertActive();
        }
      }
      this.restartAttempts += 1;
      try {
        await this.start();
        this.restartAttempts = 0;
        this.nextRestartAt = undefined;
        return;
      } catch (error) {
        lastError = error;
        if (this.disposed) throw error;
        // §2: a protocol version mismatch is fatal and loud. Retrying it would
        // only re-spawn an executor that has already declared it cannot talk.
        if (isMakaCuLifecycleError(error, 'service_mismatch')) {
          this.state = 'unavailable';
          throw error;
        }
        const backoff =
          (this.opts.restartBackoffMs ?? DEFAULT_RESTART_BACKOFF_MS) *
          2 ** (this.restartAttempts - 1);
        this.nextRestartAt = Date.now() + backoff;
      }
    }
    this.state = 'unavailable';
    this.emitRelease('restart_exhausted', [], false, false);
    throw new MakaCuLifecycleError(
      'service_unavailable',
      // §1 sends every executor diagnostic to stderr, so the tail is the only
      // account of why an executor that never completed a handshake gave up.
      `maka-cu restart budget exhausted: ${
        lastError instanceof Error ? lastError.message : String(lastError)
      }${this.stderrTail ? ` (stderr: ${this.stderrTail.slice(-400)})` : ''}`,
      this.generation,
    );
  }

  private async start(): Promise<void> {
    this.assertActive();
    this.state = 'starting';
    const executablePath = await this.verifyExecutable();
    // §8/§11: after a crash the executor's images leak, and the host owns the
    // directory, so it is purged here rather than trusted to be empty.
    await rm(this.opts.imageDir, { recursive: true, force: true });
    await mkdir(this.opts.imageDir, { recursive: true, mode: 0o700 });
    this.assertActive();

    // `host` is not optional. The same executable also serves `doctor`,
    // `list-apps` and `snapshot` for a human at a terminal, and a bare
    // invocation prints help and exits — which is what happened the first time
    // this ran against a real executor: the child died before the handshake and
    // the host reported an exhausted restart budget rather than a wrong argv.
    // §11 said "spawns the executor as a direct child" and did not say with
    // what, so the two sides each picked, and disagreed.
    const child = spawn(executablePath, ['host'], {
      stdio: ['pipe', 'pipe', 'pipe'],
      // §13: no env-var behaviour switches. Everything behavioural is a
      // `host.hello` parameter, so the wire says what the executor will do.
      env: this.childEnv,
    });
    this.generation += 1;
    this.child = child;
    this.buffer = '';
    this.stderrTail = '';
    this.nonJsonLines = 0;
    this.handshake = undefined;
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk: string) => this.onStdout(child, chunk));
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', (chunk: string) => this.onStderr(child, chunk));
    child.stdin.on('error', () => this.onExit(child, 'child_exit'));
    child.on('exit', () => this.onExit(child, 'child_exit'));
    child.on('error', () => this.onExit(child, 'child_exit'));

    try {
      this.handshake = await this.hello();
      this.assertActive();
      this.state = 'ready';
    } catch (error) {
      this.kill('child_exit');
      throw error;
    }
  }

  private async hello(): Promise<MakaCuHandshake> {
    const timeoutMs = this.opts.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;
    const protocol = this.opts.protocolVersion ?? MAKA_CU_PROTOCOL_VERSION;
    const response = await this.request(
      'host.hello',
      {
        protocol,
        host: { name: 'maka', version: this.opts.hostVersion },
        hostPid: process.pid,
        imageDir: this.opts.imageDir,
        allowGlobalPointer: MAKA_CU_ALLOW_GLOBAL_POINTER,
      },
      { timeoutMs },
    );
    if (response.error) {
      const supported = response.error.data?.supported;
      throw new MakaCuLifecycleError(
        response.error.code === MAKA_CU_RPC_ERROR.protocolVersionMismatch
          ? 'service_mismatch'
          : 'service_unavailable',
        `host.hello rejected: ${response.error.message}${
          Array.isArray(supported) ? ` (executor supports ${supported.join(', ')})` : ''
        }`,
        this.generation,
      );
    }
    const envelope = readEnvelope('host.hello', response.result);
    if (!envelope.ok) {
      throw new MakaCuLifecycleError(
        'service_mismatch',
        `host.hello refused: ${envelope.error.code}`,
        this.generation,
      );
    }
    if (envelope.protocol !== protocol) {
      throw new MakaCuLifecycleError(
        'service_mismatch',
        `executor answered protocol ${String(envelope.protocol)}, host speaks ${protocol}`,
        this.generation,
      );
    }
    return {
      executor: readExecutorInfo(envelope.executor),
      pid: readNumber(envelope.pid, 'pid'),
      capabilities: readCapabilities(envelope.capabilities),
      limits: readLimits(envelope.limits),
    };
  }

  private async verifyExecutable(): Promise<string> {
    try {
      const resolved = await realpath(this.opts.binaryPath);
      await access(resolved, fsConstants.X_OK);
      if (this.opts.expectedBinarySha256) {
        const actual = createHash('sha256')
          .update(await readFile(resolved))
          .digest('hex');
        if (actual !== this.opts.expectedBinarySha256) {
          throw new Error(
            `binary sha256 mismatch: expected ${this.opts.expectedBinarySha256}, got ${actual}`,
          );
        }
      }
      return resolved;
    } catch (error) {
      this.state = 'unavailable';
      throw new MakaCuLifecycleError(
        'service_mismatch',
        error instanceof Error ? error.message : String(error),
        this.generation,
      );
    }
  }

  private onStdout(child: ChildProcessWithoutNullStreams, chunk: string): void {
    if (this.child !== child) return;
    const rest = decodeJsonLines(this.buffer, chunk, {
      maxBufferBytes: MAX_STDOUT_BUFFER,
      onOverflow: () => this.kill('child_exit'),
      onMessage: (value) => {
        const message = value as MakaCuRpcResponse;
        // §1: responses MAY arrive out of order; correlation is by id only.
        if (typeof message.id !== 'number') return;
        this.pending.get(message.id)?.resolve(message);
      },
      onNonJsonLine: () => {
        this.nonJsonLines += 1;
        if (this.nonJsonLines >= MAX_NON_JSON_LINES) this.kill('protocol_violation');
      },
    });
    if (this.child === child) this.buffer = rest;
  }

  private onStderr(child: ChildProcessWithoutNullStreams, chunk: string): void {
    if (this.child !== child) return;
    this.stderrTail = (this.stderrTail + chunk).slice(-STDERR_TAIL_CAP);
  }

  /** §11: classify in-flight requests by stage. Delivered means outcome unknown. */
  private onExit(
    child: ChildProcessWithoutNullStreams,
    reason: MakaCuReleaseEvent['reason'],
  ): void {
    if (this.child !== child) return;
    const requests = [...this.pending.values()];
    this.pending.clear();
    const potentiallyDelivered = requests.filter(
      (request) => request.stage === 'writing' || request.stage === 'delivered',
    );
    const sessionIds = potentiallyDelivered.flatMap((request) =>
      request.sessionId ? [request.sessionId] : [],
    );
    for (const request of requests) {
      request.reject(
        request.stage === 'writing' || request.stage === 'delivered'
          ? new MakaCuLifecycleError(
              'outcome_unknown',
              // Why the child is gone, not just that it is. The host kills it
              // on its own deadline as well as on a crash, and both arrived
              // here saying "exited after request delivery" — which reads as
              // "the executor died" and sends whoever is looking at it to the
              // wrong side. Observing an app whose front window is a file
              // dialog costs about eighteen seconds against a twenty-second
              // deadline, so this is the message a busy machine produces, for
              // an executor that was alive and working.
              reason === 'request_timeout'
                ? 'maka-cu did not answer within the host deadline and was terminated'
                : 'maka-cu exited after request delivery',
              this.generation,
              request.stage,
            )
          : new MakaCuLifecycleError(
              'service_unavailable',
              reason === 'request_timeout'
                ? 'maka-cu did not answer within the host deadline and was terminated'
                : 'maka-cu exited before request delivery',
              this.generation,
              request.stage,
            ),
      );
    }
    this.child = undefined;
    this.buffer = '';
    this.handshake = undefined;
    if (!this.disposed) {
      this.state = 'idle';
      this.nextRestartAt = Date.now() + (this.opts.restartBackoffMs ?? DEFAULT_RESTART_BACKOFF_MS);
    }
    this.emitRelease(reason, sessionIds, potentiallyDelivered.length > 0, true);
  }

  private notify(method: string, params?: unknown): void {
    try {
      this.child?.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`);
    } catch {
      // The child event handlers own teardown.
    }
  }

  private request(
    method: string,
    params: unknown,
    opts: { timeoutMs?: number; signal?: AbortSignal } = {},
  ): Promise<MakaCuRpcResponse> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const child = this.child;
      if (!child || child.killed) {
        reject(
          new MakaCuLifecycleError(
            'service_unavailable',
            'maka-cu is not running',
            this.generation,
            'queued',
          ),
        );
        return;
      }
      let timer: ReturnType<typeof setTimeout> | undefined;
      let onAbort: (() => void) | undefined;
      const cleanup = () => {
        if (timer) clearTimeout(timer);
        if (onAbort && opts.signal) opts.signal.removeEventListener('abort', onAbort);
        this.pending.delete(id);
      };
      const entry: PendingRequest = {
        sessionId: this.sessionContext.getStore(),
        stage: 'queued',
        resolve: (response) => {
          entry.stage = 'settled';
          cleanup();
          resolve(response);
        },
        reject: (error) => {
          cleanup();
          reject(error);
        },
      };
      this.pending.set(id, entry);
      if (opts.signal) {
        if (opts.signal.aborted) {
          entry.reject(
            new MakaCuLifecycleError(
              'aborted',
              'request aborted before delivery',
              this.generation,
              entry.stage,
            ),
          );
          return;
        }
        // §7.2: a delivered request is cancelled by asking, not by killing the
        // child. The executor answers `aborted` if it has not dispatched yet and
        // the real outcome if it has — killing here would turn every cancelled
        // pre-dispatch request into outcome_unknown and lose the action's fate.
        onAbort = () => {
          if (entry.stage === 'writing' || entry.stage === 'delivered') {
            this.notify('$/cancel', { id });
            return;
          }
          entry.reject(
            new MakaCuLifecycleError(
              'aborted',
              'request aborted before delivery',
              this.generation,
              entry.stage,
            ),
          );
        };
        opts.signal.addEventListener('abort', onAbort, { once: true });
      }
      const timeoutMs = opts.timeoutMs ?? this.opts.timeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
      timer = setTimeout(() => {
        // §7.3: the host owns the deadline and enforces it with `$/cancel`,
        // followed by teardown when the request had already been delivered.
        const delivered = entry.stage === 'writing' || entry.stage === 'delivered';
        this.notify('$/cancel', { id });
        if (delivered) {
          this.kill('request_timeout');
          return;
        }
        entry.reject(
          new MakaCuLifecycleError(
            'service_unavailable',
            `maka-cu ${method} timed out before delivery`,
            this.generation,
            entry.stage,
          ),
        );
      }, timeoutMs);
      try {
        entry.stage = 'writing';
        child.stdin.write(
          `${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`,
          (error) => {
            if (error) {
              entry.reject(error);
              return;
            }
            if (this.pending.has(id)) entry.stage = 'delivered';
          },
        );
      } catch (error) {
        entry.reject(error as Error);
      }
    });
  }

  /**
   * One request/response round trip. JSON-RPC `error` becomes MakaCuRpcError;
   * the `result` envelope is handed back whole so the caller can act on the
   * domain arm (§1.1) without this layer inventing a meaning for it.
   */
  async call(method: string, params: unknown, signal?: AbortSignal): Promise<MakaCuEnvelope> {
    this.assertActive();
    await this.ensureStarted(signal);
    this.assertActive();
    const response = await this.request(method, params, { ...(signal ? { signal } : {}) });
    if (response.error) throw new MakaCuRpcError(method, response.error);
    return readEnvelope(method, response.result);
  }

  clearSession(sessionId: string): void {
    const ownsPending = [...this.pending.values()].some(
      (request) =>
        request.sessionId === sessionId &&
        (request.stage === 'writing' || request.stage === 'delivered'),
    );
    if (ownsPending) {
      this.kill('session_cleared');
      return;
    }
    this.emitRelease('session_cleared', [sessionId], false, false);
  }

  /** The executor stated a path or a shape the protocol forbids (§6.3). */
  reportProtocolViolation(): void {
    this.kill('protocol_violation');
  }

  private kill(reason: MakaCuReleaseEvent['reason']): void {
    const child = this.child;
    if (!child) return;
    child.kill('SIGKILL');
    this.onExit(child, reason);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.state = 'disposed';
    const child = this.child;
    const grace = this.handshake?.limits.shutdownGraceMs ?? FALLBACK_SHUTDOWN_GRACE_MS;
    const purge = () => {
      void rm(this.opts.imageDir, { recursive: true, force: true }).catch(() => {
        // Image-directory cleanup must not make host shutdown fail; the next
        // spawn purges it again anyway (§8).
      });
    };
    if (child) {
      // §11: SIGTERM lets in-flight mutating dispatches finish and every session
      // end — which is what removes the executor-drawn cursor and the images.
      // SIGKILL first would leak both.
      try {
        child.kill('SIGTERM');
      } catch {
        // Already gone.
      }
      const timer = setTimeout(() => {
        try {
          child.kill('SIGKILL');
        } catch {
          // Already gone.
        }
        purge();
      }, grace);
      timer.unref?.();
      child.once('exit', () => {
        clearTimeout(timer);
        purge();
      });
      this.onExit(child, 'disposed');
    } else {
      purge();
    }
    try {
      this.emitRelease('disposed', [], false, false);
    } catch {
      // Release observers must not interrupt process cleanup.
    }
  }
}

function readNumber(value: unknown, what: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new Error(`maka-cu host.hello: ${what} is not a finite number`);
  }
  return value;
}

function readStringArray(value: unknown, what: string): string[] {
  if (!Array.isArray(value) || value.some((entry) => typeof entry !== 'string')) {
    throw new Error(`maka-cu host.hello: ${what} is not a string array`);
  }
  return value as string[];
}

function readExecutorInfo(value: unknown): MakaCuExecutorInfo {
  const record = (value ?? {}) as Record<string, unknown>;
  if (typeof record.name !== 'string' || typeof record.version !== 'string') {
    throw new Error('maka-cu host.hello: executor identity is missing');
  }
  return {
    name: record.name,
    version: record.version,
    ...(typeof record.commit === 'string' ? { commit: record.commit } : {}),
  };
}

function readCapabilities(value: unknown): MakaCuCapabilities {
  const record = (value ?? {}) as Record<string, unknown>;
  if (typeof record.captureStream !== 'boolean') {
    throw new Error('maka-cu host.hello: capabilities.captureStream is missing');
  }
  return {
    captureStream: record.captureStream,
    elementActions: readStringArray(record.elementActions, 'capabilities.elementActions'),
    pointActions: readStringArray(record.pointActions, 'capabilities.pointActions'),
    keyActions: readStringArray(record.keyActions, 'capabilities.keyActions'),
    imageFormats: readStringArray(record.imageFormats, 'capabilities.imageFormats'),
  };
}

function readLimits(value: unknown): MakaCuLimits {
  const record = (value ?? {}) as Record<string, unknown>;
  return {
    snapshotsPerSession: readNumber(record.snapshotsPerSession, 'limits.snapshotsPerSession'),
    snapshotTtlMs: readNumber(record.snapshotTtlMs, 'limits.snapshotTtlMs'),
    maxElements: readNumber(record.maxElements, 'limits.maxElements'),
    maxDepth: readNumber(record.maxDepth, 'limits.maxDepth'),
    maxTextChars: readNumber(record.maxTextChars, 'limits.maxTextChars'),
    maxResponseBytes: readNumber(record.maxResponseBytes, 'limits.maxResponseBytes'),
    settleCeilingMs: readNumber(record.settleCeilingMs, 'limits.settleCeilingMs'),
    shutdownGraceMs: readNumber(record.shutdownGraceMs, 'limits.shutdownGraceMs'),
    imageDirBudgetBytes: readNumber(record.imageDirBudgetBytes, 'limits.imageDirBudgetBytes'),
  };
}
