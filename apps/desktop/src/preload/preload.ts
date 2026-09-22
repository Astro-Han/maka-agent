/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import { invokeWhenReady, sendWhenReady } from './bootstrap-invoke.js';

import type {
  SessionBundleExportIpcResult,
  SessionBundleImportIpcResult,
} from './bridge-contract.js';

import type {
  PrepareAttachmentsResult,
} from '../shared/attachment-ingest-result.js';
import type { SessionObservationMessage } from '../shared/session-execution-projection.js';
import { contextBridge, ipcRenderer } from 'electron';
import { HOST_OPERATION_SPECS } from '@maka/runtime-host/protocol';
import { workHubControlBridge } from './workhub-control.js';
import { workHubPresentationBridge } from './workhub-presentation.js';
import {
  isRuntimeHostProfileKind,
  type RuntimeHostProfileKind,
} from '@maka/runtime-host/profile-kind';
import { AttachmentIngestBlockedError } from '@maka/core/attachments';
import { encodeIngestItems } from './attachment-ingest-payload.js';
import { createThreadSearchClient } from './multi-host-thread-search.js';
import { releaseSessionObservation } from './session-observation-release.js';
import { subscribeClientEvents } from './client-plugin-events.js';
import type {
  MakaBridge,
  OnboardingSnapshot,
  DesktopTaskSubmissionReadinessRequest,
  PermissionActionResult,
  PermissionOverlayStartResult,
  RendererIngestInput,
  DesktopBranchFromTurnInput,
  DesktopSideConversationBranchResult,
  DesktopSessionStopResult,
  DesktopReviseBeforeTurnInput,
  AppUpdateInstallRequest,
  AppUpdateInstallResult,
  AppUpdateStatus,
  WindowCommand,
  PetPackChangedEvent,
  WorkBoardChangedEvent,
  DesktopRuntimeHostProfileAddInput,
  DesktopRuntimeHostProfileChangedEvent,
  DesktopRuntimeHostProfileSnapshot,
  DesktopRuntimeHostSshTerminalEvent,
  DesktopRuntimeHostSshTerminalSnapshot,
  DesktopRuntimeHostOnboardingInput,
  DesktopRuntimeHostOnboardingSnapshot,
  DesktopRuntimeHostManagementAction,
  DesktopRuntimeHostManagementResponse,
  DesktopRuntimeHostManagementProgress,
  DesktopRuntimeHostAccessSnapshot,
  DesktopNewTaskCatalog,
  DesktopNewTaskHost,
  DesktopNewTaskHostRef,
  DesktopNewTaskTarget,
  DesktopRuntimeHostRef,
  DesktopOAuthLoginTarget,
  DesktopOAuthAuthorizationResult,
  DesktopProjectSnapshot,
  DesktopAppInfo,
  DesktopSessionTracePage,
  DesktopSessionUsageSummary,
  AppIconImportResult,
  AppIconRemoveResult,
  AppIconSelectResult,
} from './bridge-contract.js';
import type { ExternalSessionImportIpcResult } from './external-session-import-result.js';
import type { RuntimeHostObservationIpcResult } from '../shared/runtime-host-observation-ipc.js';
import {
  projectDesktopExternalSessionCatalogItem,
  type DesktopExternalSessionCatalogItem,
  type DesktopHostExternalSessionCatalogItem,
} from './external-session-catalog.js';
import {
  assertDesktopTranscriptBatch,
  type DesktopTranscriptBatch,
  type DesktopTranscriptHandle,
  type DesktopTranscriptOpenMode,
  type DesktopTranscriptOpenResult,
} from './transcript-contract.js';
import {
  adoptTranscriptIdentity,
  type DesktopTranscriptIdentity,
} from './transcript-identity.js';
import type {
  DesktopDiagnosticInput,
  DesktopErrorDiagnosticWireInput,
  DesktopExecutionDiagnosticTarget,
  DesktopManualDiagnosticTarget,
  DesktopManualDiagnosticWireInput,
} from './diagnostics-contract.js';
import type { ConnectionEvent } from '@maka/core/connections';
import type {
  ConnectionTestResult,
  CreateConnectionInput,
  LlmConnection,
  ModelDiscoveryResult,
  ModelInfo,
  UpdateConnectionInput,
} from '@maka/core/llm-connections';
import type {
  AppIcon,
  AppIconChoice,
  AppIconTarget,
  AppSettings,
  RuntimeHostAppSettings,
  RuntimeHostSettingsUpdateGuard,
  SettingsTestResult,
  UpdateAppSettingsInput,
  UpdateAppSettingsResult,
  UsageRange,
  UsageStats,
  UsageScreenQuery,
  UsageScreenRequest,
  UsageScreenResult,
  UsageScreenFailure,
  ThemePreference,
} from '@maka/core/settings';
import type { BotProvider } from '@maka/core/bot-chat-settings';
import type { BotOnboardingSnapshot, BotOnboardingStartInput } from '@maka/core/bot-onboarding';
import type { HealthSnapshot } from '@maka/core/health';
import {
  createRuntimeHostSessionCatalogRefresher,
  recordObservedRuntimeHostSessionAuthority,
  reconcileRuntimeHostSessionCatalog,
  resolveRuntimeHostSessionCatalog,
  type RuntimeHostSessionCatalogCoverage,
} from './runtime-host-session-catalog.js';
import { collectAvailablePendingTurnRequests } from './runtime-host-turn-request-inbox.js';
import type { ExecutionBoundaryReadModel, SandboxBoundaryResponse } from '@maka/core/sandbox-boundary';
import type { ClientCapabilityResponse } from '@maka/core/client-capability-grant';
import type {
  ActiveInteractionRequestEvent,
  MessageContent,
  SessionCommand,
  SessionEvent,
  ShellRunUpdate,
} from '@maka/core/events';
import type { UserQuestionResponse } from '@maka/core/user-question';
import type { InteractionFormResponse } from '@maka/core/interaction';
import type { SandboxMode } from '@maka/core/permission';
import type { CollaborationMode } from '@maka/core/collaboration';
import type { OrchestrationMode } from '@maka/core/orchestration';

import type { TurnOrchestration, SessionListFilter, RegenerateTurnInput } from '@maka/core/runtime-inputs';
import type { PlanSessionState } from '@maka/core/plan';
import type { SearchErrorReason, SearchResult } from '@maka/core/search';
import type {
  SessionCatalogSummary,
  SessionChangedEvent,
  SessionSummary,
  StoredMessage,
  TurnRecord,
} from '@maka/core/session';
import type { ThinkingLevel } from '@maka/core/model-thinking';
import type { E2eFixtureState } from '@maka/core/e2e-fixture';
import type {
  GitReviewReadResult,
  GitReviewSource,
} from '@maka/core/git-review';
import type {
  ArtifactBinaryReadResult,
  ArtifactDescriptor,
  ArtifactSaveResult,
  ArtifactTextReadResult,
} from '@maka/core/artifacts';
import type { CapabilitySnapshotCollection, PermissionSnapshot } from '@maka/core/capabilities';
import type { LocalMemoryState } from '@maka/core/local-memory';
import type { SubscriptionActionResult } from '@maka/core/oauth-subscription';
import type { ProjectRecord } from '@maka/core/project';
import type {
  DailyReviewArchive,
  DailyReviewArchiveSummary,
  DailyReviewConfig,
  DailyReviewRange,
  DailyReviewSummary,
} from '@maka/core/daily-review';
import type { BrowserState, BrowserViewRect } from '@maka/core/browser';
import { createBrowserSelectionCoordinator } from './browser-selection.js';
import {
  isSessionTrace,
} from '@maka/core/session-trace';
import type { ContextDiagnosticsResult } from '@maka/runtime-host/protocol';
import {
  DAILY_REVIEW_RANGES,
  normalizeDailyReviewConfig,
} from '@maka/core/daily-review';
import type {
  AgentGraphClientSnapshot,
  AgentGraphClientSnapshotOptions,
  AgentGraphOperatorInspection,
} from '@maka/runtime/stream-graph-read-model';
import type { BotStatus, WechatBridgeQrCodeResult } from '@maka/runtime/bots';
import type { ShellRunPtyDataEvent, ShellRunPtySnapshot } from '@maka/runtime/shell-run-contract';
import type { GoalState } from '@maka/runtime/goal-state';
import type { ConfigCategory } from '@maka/storage/config-transfer';
import {
  SENSITIVE_PLACEHOLDER,
  type TestProxyInput,
} from '@maka/core/settings/network-settings';
import type { Result } from '@maka/core/result';
import type { CreateSessionRequestInput } from '@maka/core/runtime-inputs';
import type {
  McpConfigAddResult,
  McpConfigImportResult,
  McpConfigFile,
  McpServerConfig,
  McpServerStatus,
  McpTestResult,
} from '@maka/core/mcp';
import type { AttachmentRef, InlineReference, QuoteRef } from '@maka/core/events';
import type { OnboardingMilestoneId } from '@maka/core/onboarding';
import {
  decodeSharedSessionCatalogProjection,
  type OperationInput,
  type OperationOutcome,
  type OperationOutput,
  type CollaborationTurnRequestQueryResult,
  type CollaborationTurnRequestWithdrawResult,
  type SessionTurnAccessRequest,
} from '@maka/runtime-host/protocol';
import type { PlanControlIpcResult } from '../shared/plan-mode-ipc.js';
import type { AgentGraphEpochDirectory } from '@maka/runtime-host/client';
import {
  desktopSessionKey,
  parseDesktopSessionKey,
  requireDesktopTargetScope,
  type DesktopTargetScope,
} from '../shared/runtime-host-identity.js';
import type { GoalArmOutcome, GoalArmRequest } from '../shared/goal-arm.js';
import {
  invokeProjectedSessionRuntimeHost as invokeProjectedSessionRuntimeHostBridge,
  projectProtocolSessionIds,
} from './projected-session-runtime-host.js';
import {
  hostAttachmentRefs,
  projectDesktopAttachmentRefs,
  projectDesktopDailyReviewSummary,
  projectDesktopSessionEvent,
  projectDesktopSessionSummary,
  projectDesktopStoredMessage,
  projectDesktopTurnRecord,
  projectDesktopUsageStats,
  projectDesktopUsageActivity,
  type DesktopSessionSummary,
  type DesktopSessionSummaryInput,
  type DesktopSessionUpdateResult,
} from '../shared/desktop-session-projection.js';
import { projectDesktopSharedSessionSummary } from '../shared/shared-session-catalog-projection.js';

let activeRuntimeHost: DesktopTargetScope | undefined;
let activeRuntimeHostGeneration = 0;
let newTaskCatalogGeneration = 0;
type RuntimeHostScopeKey = string;
const runtimeHostScopes = new Map<string, DesktopTargetScope>();
const runtimeHostProfiles = new Map<string, string>();
const runtimeHostMetadata = new Map<
  string,
  {
    readonly profileId: string;
    readonly profileName: string;
    readonly profileKind: RuntimeHostProfileKind;
    readonly profileAccess: 'owner' | 'session_guest';
  }
>();
const runtimeHostSessionProfiles = new Map<string, string>();
let lastDesktopSessionCatalog: RuntimeHostSessionCatalogCoverage = {
  sessions: [],
  completeHostIds: [],
};
const newTaskChangeListeners = new Set<() => void>();
let previousMainProcessInterruptionRead: Promise<boolean> | undefined;

function runtimeHostScopeKey(scope: DesktopTargetScope): RuntimeHostScopeKey {
  return `${scope.hostId}\u0000${scope.targetEpoch}`;
}

function runtimeHostMetadataFor(scope: DesktopTargetScope) {
  return runtimeHostMetadata.get(runtimeHostScopeKey(scope));
}

function observeRuntimeHostSessionScope(scope: DesktopTargetScope, sessionId: string): {
  readonly sessionId: string;
  readonly authorityAccepted: boolean;
} {
  const projected = desktopSessionKey({ hostId: scope.hostId, sessionId });
  const profileId = runtimeHostMetadataFor(scope)?.profileId;
  if (!profileId) throw new Error('Desktop Runtime Host metadata is unavailable');
  return {
    sessionId: projected,
    authorityAccepted: recordObservedRuntimeHostSessionAuthority(
      runtimeHostSessionProfiles,
      projected,
      profileId,
    ),
  };
}

function recordRuntimeHostSessionScope(scope: DesktopTargetScope, sessionId: string): string {
  return observeRuntimeHostSessionScope(scope, sessionId).sessionId;
}

type RuntimeHostProfileWireEvent = DesktopRuntimeHostProfileChangedEvent;

ipcRenderer.on(
  'runtime-host-profiles:changed',
  (_event, change: RuntimeHostProfileWireEvent) => {
    const previousScopeKey = runtimeHostProfiles.get(change.profileId);
    const nextScope = change.hostId
      ? { hostId: change.hostId, targetEpoch: change.epoch }
      : undefined;
    const nextScopeKey = nextScope ? runtimeHostScopeKey(nextScope) : undefined;
    if (
      previousScopeKey &&
      (change.removed || (nextScopeKey !== undefined && previousScopeKey !== nextScopeKey))
    ) {
      runtimeHostScopes.delete(previousScopeKey);
      runtimeHostMetadata.delete(previousScopeKey);
      if (change.removed) {
        runtimeHostProfiles.delete(change.profileId);
      }
    }
    if (nextScope && nextScopeKey) {
      runtimeHostScopes.set(nextScopeKey, nextScope);
      runtimeHostProfiles.set(change.profileId, nextScopeKey);
      runtimeHostMetadata.set(nextScopeKey, {
        profileId: change.profileId,
        profileName: change.profileName,
        profileKind: change.profileKind,
        profileAccess: change.profileAccess,
      });
      if (change.isDefault) activeRuntimeHost = nextScope;
    } else if (change.isDefault) {
      activeRuntimeHost = undefined;
    }
    if (
      change.hostId ||
      change.removed ||
      change.isDefault ||
      change.readiness === 'unavailable'
    ) {
      activeRuntimeHostGeneration += 1;
    }
    // Guest mounts can only participate in their shared Sessions. Their
    // reconnects cannot change the Hosts/projects available for a new task.
    if (change.profileAccess === 'owner') {
      newTaskCatalogGeneration += 1;
      for (const listener of newTaskChangeListeners) listener();
    }
  },
);

function recordRuntimeHostIdentity(value: unknown): {
  readonly scope: DesktopTargetScope;
  readonly readiness: 'ready' | 'reconnecting';
} {
  const scope = requireDesktopTargetScope(value);
  const metadata = value as {
    profileId?: unknown;
    profileName?: unknown;
    profileKind?: unknown;
    profileAccess?: unknown;
    readiness?: unknown;
  };
  if (
    typeof metadata.profileId !== 'string' ||
    typeof metadata.profileName !== 'string' ||
    !isRuntimeHostProfileKind(metadata.profileKind) ||
    (metadata.profileAccess !== 'owner' && metadata.profileAccess !== 'session_guest') ||
    (metadata.readiness !== 'ready' && metadata.readiness !== 'reconnecting')
  ) {
    throw new Error('Desktop Runtime Host identity is invalid');
  }
  const scopeKey = runtimeHostScopeKey(scope);
  runtimeHostScopes.set(scopeKey, scope);
  runtimeHostProfiles.set(metadata.profileId, scopeKey);
  runtimeHostMetadata.set(scopeKey, {
    profileId: metadata.profileId,
    profileName: metadata.profileName,
    profileKind: metadata.profileKind,
    profileAccess: metadata.profileAccess,
  });
  return { scope, readiness: metadata.readiness };
}

async function runtimeHostScopeList(): Promise<readonly DesktopTargetScope[]> {
  while (true) {
    const generation = activeRuntimeHostGeneration;
    const identities: unknown = await invokeWhenReady('runtime-host:identities');
    if (generation !== activeRuntimeHostGeneration) continue;
    if (!Array.isArray(identities)) {
      throw new Error('Desktop Runtime Host identities are unavailable');
    }
    const authoritativeScopeKeys = new Set<RuntimeHostScopeKey>();
    const readyScopes: DesktopTargetScope[] = [];
    for (const identity of identities) {
      const { scope, readiness } = recordRuntimeHostIdentity(identity);
      authoritativeScopeKeys.add(runtimeHostScopeKey(scope));
      if (readiness === 'ready') readyScopes.push(scope);
    }
    for (const scopeKey of runtimeHostScopes.keys()) {
      if (authoritativeScopeKeys.has(scopeKey)) continue;
      runtimeHostScopes.delete(scopeKey);
      runtimeHostMetadata.delete(scopeKey);
    }
    return readyScopes;
  }
}

async function readyOwnerRuntimeHostScopes(): Promise<readonly DesktopTargetScope[]> {
  return (await runtimeHostScopeList()).filter(
    (scope) => runtimeHostMetadataFor(scope)?.profileAccess === 'owner',
  );
}

async function runtimeHostSessionRef(sessionId: string): Promise<{
  readonly scope: DesktopTargetScope;
  readonly sessionId: string;
}> {
  const ref = parseDesktopSessionKey(sessionId);
  // The profile maps are already kept current by the identities push channel;
  // only pull on a miss so routine calls (every terminal keystroke) stay local.
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt > 0) await runtimeHostScopeList();
    const recordedProfileId = runtimeHostSessionProfiles.get(sessionId);
    let scope: DesktopTargetScope | undefined;
    if (recordedProfileId) {
      const scopeKey = runtimeHostProfiles.get(recordedProfileId);
      const recorded = scopeKey ? runtimeHostScopes.get(scopeKey) : undefined;
      if (recorded?.hostId === ref.hostId) scope = recorded;
    } else {
      const candidates = [...runtimeHostScopes.values()].filter(
        ({ hostId }) => hostId === ref.hostId,
      );
      if (candidates.length === 1) scope = candidates[0];
    }
    if (scope) return { scope, sessionId: ref.sessionId };
  }
  throw new Error('The Runtime Host for this task is unavailable');
}

type DiagnosticRuntimeHostResolution<TTarget extends 'default' | 'task'> = {
  readonly hostTarget: TTarget;
  readonly scope?: DesktopTargetScope;
};

type TaskDiagnosticRuntimeHostResolution = DiagnosticRuntimeHostResolution<'task'>;
type ManualDiagnosticRuntimeHostResolution = DiagnosticRuntimeHostResolution<'default' | 'task'>;

type ManualDiagnosticHostSelector =
  | { readonly kind: 'profile'; readonly profileId: string }
  | { readonly kind: 'session'; readonly sessionId: string };

async function resolveManualDiagnosticRuntimeHost(
  value: DesktopManualDiagnosticTarget | undefined,
): Promise<ManualDiagnosticRuntimeHostResolution> {
  if (value === undefined) return { hostTarget: 'default' };
  const target = parseDiagnosticTarget(value);
  if (target.execution) {
    throw new TypeError('Manual Desktop diagnostics do not accept execution targets');
  }
  return resolveTaskDiagnosticRuntimeHost(target.selector);
}

async function resolveTaskDiagnosticRuntimeHost(
  selector: ManualDiagnosticHostSelector,
): Promise<TaskDiagnosticRuntimeHostResolution> {
  try {
    await runtimeHostScopeList();
  } catch {
    return { hostTarget: 'task' };
  }
  if (selector.kind === 'session') {
    try {
      return { hostTarget: 'task', scope: (await runtimeHostSessionRef(selector.sessionId)).scope };
    } catch {
      return { hostTarget: 'task' };
    }
  }
  const scope = runtimeHostScopes.get(runtimeHostProfiles.get(selector.profileId) ?? '');
  return { hostTarget: 'task', ...(scope ? { scope } : {}) };
}

function parseDiagnosticTarget(value: unknown): {
  readonly selector: ManualDiagnosticHostSelector;
  readonly execution?: DesktopExecutionDiagnosticTarget;
} {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('Invalid Desktop diagnostic target');
  }
  const record = value as Record<string, unknown>;
  if (
    Object.keys(record).length === 1 &&
    Object.hasOwn(record, 'sessionId') &&
    typeof record.sessionId === 'string' &&
    Buffer.byteLength(record.sessionId, 'utf8') <= 512
  ) {
    return {
      selector: { kind: 'session', sessionId: record.sessionId },
    };
  }
  if (
    Object.keys(record).length === 1 &&
    Object.hasOwn(record, 'profileId') &&
    typeof record.profileId === 'string' &&
    record.profileId.length > 0 &&
    !/[\u0000-\u001f\u007f]/.test(record.profileId) &&
    Buffer.byteLength(record.profileId, 'utf8') <= 512
  ) {
    return { selector: { kind: 'profile', profileId: record.profileId } };
  }
  const executionKeys = ['sessionId', 'turnId', 'eventId'] as const;
  if (
    Object.keys(record).length === executionKeys.length &&
    executionKeys.every((key) => Object.hasOwn(record, key)) &&
    typeof record.sessionId === 'string' &&
    Buffer.byteLength(record.sessionId, 'utf8') <= 512 &&
    typeof record.turnId === 'string' &&
    Buffer.byteLength(record.turnId, 'utf8') <= 512 &&
    typeof record.eventId === 'string' &&
    Buffer.byteLength(record.eventId, 'utf8') <= 512
  ) {
    return {
      selector: { kind: 'session', sessionId: record.sessionId },
      execution: {
        sessionId: parseDesktopSessionKey(record.sessionId).sessionId,
        turnId: record.turnId,
        eventId: record.eventId,
      },
    };
  }
  throw new TypeError('Invalid Desktop diagnostic target');
}

async function activeRuntimeHostRef(): Promise<DesktopTargetScope> {
  while (!activeRuntimeHost) {
    const generation = activeRuntimeHostGeneration;
    const snapshot = await invokeWhenReady('runtime-host:activeIdentity');
    if (generation !== activeRuntimeHostGeneration) continue;
    activeRuntimeHost = recordRuntimeHostIdentity(snapshot).scope;
  }
  return activeRuntimeHost;
}

async function localRuntimeHostRef(): Promise<DesktopTargetScope> {
  const scopes = await runtimeHostScopeList();
  const scope = scopes.find(
    (candidate) => runtimeHostMetadataFor(candidate)?.profileKind === 'local',
  );
  if (!scope) throw new Error('The Local Runtime Host is unavailable');
  return scope;
}

async function runtimeHostScope(host: DesktopRuntimeHostRef): Promise<DesktopTargetScope> {
  if (!host.profileId || !host.hostId) {
    throw new Error('The Runtime Host target is invalid');
  }
  await runtimeHostScopeList();
  const currentScopeKey = runtimeHostProfiles.get(host.profileId);
  const scope = currentScopeKey ? runtimeHostScopes.get(currentScopeKey) : undefined;
  if (!scope || scope.hostId !== host.hostId) {
    throw new Error('The selected Runtime Host is no longer available');
  }
  return scope;
}

async function selectedRuntimeHostScope(
  host: DesktopRuntimeHostRef | undefined,
): Promise<DesktopTargetScope> {
  return host ? runtimeHostScope(host) : activeRuntimeHostRef();
}

async function loadNewTaskCatalog(): Promise<DesktopNewTaskCatalog> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const generation = newTaskCatalogGeneration;
    const profiles = await invokeWhenReady(
      'runtime-host-profiles:getSnapshot',
    ) as DesktopRuntimeHostProfileSnapshot;
    await runtimeHostScopeList();
    const hosts = await Promise.all(
      profiles.entries
        .filter(
          (entry) =>
            entry.enabled &&
            (entry.profile.kind !== 'remote' || entry.profile.access !== 'session_guest'),
        )
        .map(async (entry): Promise<DesktopNewTaskHost> => {
        if (entry.readiness !== 'ready' || !entry.hostId) {
          return {
            profile: entry.profile,
            readiness: entry.readiness === 'ready'
              ? 'reconnecting'
              : entry.readiness === 'disabled'
                ? 'unavailable'
                : entry.readiness,
            ...(entry.message ? { message: entry.message } : {}),
          };
        }
        const host = { profileId: entry.profile.id, hostId: entry.hostId };
        try {
          const scope = await runtimeHostScope(host);
          const [snapshot, info, settings] = await Promise.all([
            invokeWhenReady('projects:getSnapshot', scope) as Promise<DesktopProjectSnapshot>,
            invokeWhenReady('app:info', scope) as Promise<DesktopAppInfo>,
            invokeWhenReady('settings:get', scope) as Promise<AppSettings>,
          ]);
          return {
            profile: entry.profile,
            hostId: entry.hostId,
            readiness: 'ready',
            state: 'available',
            projects: snapshot.projects,
            capabilities: snapshot.capabilities,
            selectedProjectId: info.projectId,
            ...(settings.projects.defaultProjectId
              ? { defaultProjectId: settings.projects.defaultProjectId }
              : {}),
            chatDefaults: settings.chatDefaults,
            ...(snapshot.capabilities.viewClientPath && info.projectPath
              ? { projectPath: info.projectPath }
              : {}),
            ...(info.projectGit.branch ? { branch: info.projectGit.branch } : {}),
          };
        } catch (error) {
          return {
            profile: entry.profile,
            hostId: entry.hostId,
            readiness: 'ready',
            state: 'error',
            message: error instanceof Error ? error.message : String(error),
          };
        }
        }),
    );
    if (generation !== newTaskCatalogGeneration) continue;
    return { defaultProfileId: profiles.defaultProfileId, hosts };
  }
  throw new Error('Runtime Host targets changed while the new-task catalog was loading');
}

async function invokeActiveRuntimeHost<T>(channel: string, ...args: unknown[]): Promise<T> {
  return invokeWhenReady(channel, await activeRuntimeHostRef(), ...args) as Promise<T>;
}

async function invokeSelectedRuntimeHost<T>(
  host: DesktopRuntimeHostRef | undefined,
  channel: string,
  ...args: unknown[]
): Promise<T> {
  return invokeWhenReady(channel, await selectedRuntimeHostScope(host), ...args) as Promise<T>;
}

function scopedRuntimeHost(scope: DesktopTargetScope): MakaBridge['runtimeHost'] {
  return {
    query(operation, input) {
      return invokeWhenReady('runtime-host:query', scope, operation, input) as Promise<
        OperationOutput<typeof operation>
      >;
    },
    command(operation, input) {
      return invokeWhenReady('runtime-host:command', scope, operation, input) as Promise<
        OperationOutput<typeof operation>
      >;
    },
  };
}

async function invokeSessionRuntimeHost<T>(
  channel: string,
  sessionId: string,
  ...args: unknown[]
): Promise<T> {
  const session = await runtimeHostSessionRef(sessionId);
  return invokeWhenReady(channel, session.scope, session.sessionId, ...args) as Promise<T>;
}

async function invokeRuntimeHostForSession<T>(
  channel: string,
  sessionId: string,
  ...args: unknown[]
): Promise<T> {
  const session = await runtimeHostSessionRef(sessionId);
  return invokeWhenReady(channel, session.scope, ...args) as Promise<T>;
}

async function invokeProjectedSessionRuntimeHost<T>(
  channel: string,
  sessionId: string,
  ...args: unknown[]
): Promise<T> {
  return invokeProjectedSessionRuntimeHostBridge<T>(
    runtimeHostSessionRef,
    (targetChannel, scope, rawSessionId, ...targetArgs) => invokeWhenReady(
      targetChannel,
      scope,
      rawSessionId,
      ...targetArgs,
    ),
    channel,
    sessionId,
    ...args,
  );
}

async function invokeSessionUpdate(
  channel: string,
  sessionId: string,
  ...args: unknown[]
): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
  const session = await runtimeHostSessionRef(sessionId);
  const result = (await invokeWhenReady(
    channel,
    session.scope,
    session.sessionId,
    ...args,
  )) as DesktopSessionUpdateResult<DesktopSessionSummaryInput>;
  return result.ok
    ? { ok: true, session: projectSessionSummary(session.scope, result.session) }
    : result;
}

async function invokeBranchFromTurn(
  sessionId: string,
  input: DesktopBranchFromTurnInput & { sideConversation: true },
): Promise<DesktopSideConversationBranchResult>;
async function invokeBranchFromTurn(
  sessionId: string,
  input: DesktopBranchFromTurnInput & { sideConversation?: false },
): Promise<DesktopSessionSummary>;
async function invokeBranchFromTurn(
  sessionId: string,
  input: DesktopBranchFromTurnInput,
): Promise<DesktopSessionSummary | DesktopSideConversationBranchResult> {
  const ref = await runtimeHostSessionRef(sessionId);
  const result = await invokeWhenReady(
    'sessions:branchFromTurn',
    ref.scope,
    ref.sessionId,
    input,
  ) as DesktopSessionSummaryInput | { ok: true; session: DesktopSessionSummaryInput } | { ok: false; reason: string };
  if (input.sideConversation) {
    if (!('ok' in result) || result.ok === false) {
      return result as DesktopSideConversationBranchResult;
    }
    return { ok: true, session: projectCreatedSessionSummary(ref.scope, result.session) };
  }
  return projectCreatedSessionSummary(ref.scope, result as DesktopSessionSummaryInput);
}

async function invokeSessionInput<T, I extends { readonly sessionId: string }>(
  channel: string,
  input: I,
  ...args: unknown[]
): Promise<T> {
  const session = await runtimeHostSessionRef(input.sessionId);
  return invokeWhenReady(
    channel,
    session.scope,
    { ...input, sessionId: session.sessionId },
    ...args,
  ) as Promise<T>;
}

function projectSessionSummary(
  scope: DesktopTargetScope,
  session: DesktopSessionSummaryInput,
): DesktopSessionSummary {
  const projected = projectSessionCatalogSummary(scope, session);
  runtimeHostSessionProfiles.set(projected.id, projected.profileId);
  return projected;
}

function projectSessionCatalogSummary(
  scope: DesktopTargetScope,
  session: DesktopSessionSummaryInput,
): DesktopSessionSummary {
  const metadata = runtimeHostMetadataFor(scope);
  if (!metadata) throw new Error('Desktop Runtime Host metadata is unavailable');
  return projectDesktopSessionSummary(
    { ...scope, ...metadata },
    session,
  );
}

function recordSessionCatalogAuthorities(sessions: readonly DesktopSessionSummary[]): void {
  for (const session of sessions) {
    runtimeHostSessionProfiles.set(session.id, session.profileId);
  }
}

function commitDesktopSessionCatalog(catalog: RuntimeHostSessionCatalogCoverage): void {
  recordSessionCatalogAuthorities(catalog.sessions);
  lastDesktopSessionCatalog = catalog;
}

function projectOnboardingSnapshot(
  scope: DesktopTargetScope,
  snapshot: OnboardingSnapshot,
): OnboardingSnapshot {
  return {
    ...snapshot,
    sessions: snapshot.sessions.map((session) => projectSessionSummary(scope, session)),
    sessionSendOutcomes: projectOnboardingSendOutcomes(scope, snapshot.sessionSendOutcomes),
  };
}

function projectOnboardingSendOutcomes(
  scope: DesktopTargetScope,
  outcomes: OnboardingSnapshot['sessionSendOutcomes'],
): OnboardingSnapshot['sessionSendOutcomes'] {
  return Object.fromEntries(
    Object.entries(outcomes).map(([sessionId, outcome]) => [
      recordRuntimeHostSessionScope(scope, sessionId),
      outcome,
    ]),
  );
}

async function loadDesktopOnboardingSnapshot(): Promise<OnboardingSnapshot> {
  const catalogSeed = desktopSessionCatalogRefresher.beginSeed();
  const defaultScope = await activeRuntimeHostRef();
  const readyScopes = await readyOwnerRuntimeHostScopes();
  const scopes = [
    defaultScope,
    ...readyScopes.filter(
      (scope) =>
        scope.hostId !== defaultScope.hostId || scope.targetEpoch !== defaultScope.targetEpoch,
    ),
  ];
  const results = await Promise.allSettled(
    scopes.map(async (scope) => ({
      scope,
      snapshot: await invokeWhenReady('onboarding:getSnapshot', scope) as OnboardingSnapshot,
    })),
  );
  const primary = results[0];
  if (!primary || primary.status === 'rejected') {
    throw primary?.reason ?? new Error('Default Runtime Host onboarding is unavailable');
  }
  const snapshots = results.flatMap((result) =>
    result.status === 'fulfilled'
      ? [result.value]
      : [],
  );
  // A successful Owner onboarding snapshot is already an authenticated,
  // complete catalog. Seed it before the independent catalog refresh so a
  // transient sessions:list failure cannot blank the first renderer commit.
  const ownerSnapshots = snapshots.filter(
    ({ scope }) => runtimeHostMetadataFor(scope)?.profileAccess === 'owner',
  );
  const completeHostIds = [...new Set(ownerSnapshots.map(({ scope }) => scope.hostId))];
  catalogSeed.commit({
    sessions: reconcileRuntimeHostSessionCatalog(lastDesktopSessionCatalog.sessions, {
      sessions: ownerSnapshots.flatMap(({ scope, snapshot }) =>
        snapshot.sessions.map((session) => projectSessionCatalogSummary(scope, session))),
      completeHostIds,
      knownOwnerProfileIds: [...runtimeHostMetadata.values()].flatMap(
        ({ profileId, profileAccess }) => profileAccess === 'owner' ? [profileId] : [],
      ),
    }),
    completeHostIds,
  });
  const sessions = await listDesktopSessions();
  return {
    ...snapshots[0]!.snapshot,
    sessions,
    sessionSendOutcomes: Object.assign(
      {},
      ...snapshots.map(({ scope, snapshot }) =>
        projectOnboardingSendOutcomes(scope, snapshot.sessionSendOutcomes)),
    ),
  };
}

function projectShellRunUpdate(
  scope: DesktopTargetScope,
  update: ShellRunUpdate,
): ShellRunUpdate {
  const sessionId = (value: string): string =>
    recordRuntimeHostSessionScope(scope, value);
  return {
    ...update,
    sessionId: sessionId(update.sessionId),
    ownership: update.ownership.kind === 'local'
      ? update.ownership
      : update.ownership.kind === 'source_owned'
        ? {
            ...update.ownership,
            sourceSessionId: sessionId(update.ownership.sourceSessionId),
            ownerSessionId: sessionId(update.ownership.ownerSessionId),
          }
        : {
            ...update.ownership,
            sourceSessionId: sessionId(update.ownership.sourceSessionId),
          },
  };
}

function subscribeRuntimeHostEvent<T extends readonly unknown[]>(
  channel: string,
  scope: DesktopTargetScope,
  handler: (...args: T) => void,
): () => void {
  const listener = (
    _event: Electron.IpcRendererEvent,
    value: unknown,
    ...args: unknown[]
  ): void => {
    let eventScope: DesktopTargetScope;
    try {
      eventScope = requireDesktopTargetScope(value);
    } catch {
      return;
    }
    const current = runtimeHostScopes.get(runtimeHostScopeKey(scope));
    if (
      !current ||
      eventScope.hostId !== current.hostId ||
      eventScope.targetEpoch !== current.targetEpoch ||
      eventScope.targetEpoch !== scope.targetEpoch
    ) return;
    handler(...(args as unknown as T));
  };
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.off(channel, listener);
}

function subscribeEveryRuntimeHostEvent<T extends readonly unknown[]>(
  channel: string,
  handler: (scope: DesktopTargetScope, ...args: T) => void,
): () => void {
  const listener = (
    _event: Electron.IpcRendererEvent,
    value: unknown,
    ...args: unknown[]
  ): void => {
    let scope: DesktopTargetScope;
    try {
      scope = requireDesktopTargetScope(value);
    } catch {
      return;
    }
    const current = runtimeHostScopes.get(runtimeHostScopeKey(scope));
    if (!current || current.targetEpoch !== scope.targetEpoch) return;
    handler(scope, ...(args as unknown as T));
  };
  ipcRenderer.on(channel, listener);
  void runtimeHostScopeList().catch(() => undefined);
  return () => {
    ipcRenderer.off(channel, listener);
  };
}

function subscribeGuestSessionMountChanges(
  handler: () => void,
): () => void {
  const listener = (): void => handler();
  ipcRenderer.on('session-collaboration:mounts:changed', listener);
  return () => ipcRenderer.off('session-collaboration:mounts:changed', listener);
}

const desktopSessionCatalogRefresher = createRuntimeHostSessionCatalogRefresher({
  currentCatalog: () => lastDesktopSessionCatalog,
  listCatalog: async () => {
    const owners = listDesktopOwnerSessionsWithCoverage();
    return resolveRuntimeHostSessionCatalog(
      lastDesktopSessionCatalog.sessions,
      owners,
      () => [...runtimeHostMetadata.values()].flatMap(({ profileId, profileAccess }) =>
        profileAccess === 'owner' ? [profileId] : []),
      listGuestSessionMountCatalog(),
    );
  },
  commitCatalog: commitDesktopSessionCatalog,
});

function projectCreatedSessionSummary(
  scope: DesktopTargetScope,
  session: DesktopSessionSummaryInput,
): DesktopSessionSummary {
  const projected = projectSessionSummary(scope, session);
  // Guest catalog membership belongs exclusively to the retained mount
  // service. A newly created Owner Session, however, must be visible before
  // an older catalog read can settle.
  if (runtimeHostMetadataFor(scope)?.profileAccess === 'owner') {
    desktopSessionCatalogRefresher.admit(projected);
  }
  return projected;
}

async function listDesktopSessions(
  filter?: SessionListFilter,
): Promise<DesktopSessionSummary[]> {
  if (filter?.subagentParentSessionId) {
    const parent = await runtimeHostSessionRef(filter.subagentParentSessionId);
    const sessions = await invokeWhenReady(
      'sessions:list',
      parent.scope,
      { ...filter, subagentParentSessionId: parent.sessionId },
    ) as DesktopSessionSummaryInput[];
    return sessions.map((session) => projectSessionSummary(parent.scope, session));
  }
  return (await desktopSessionCatalogRefresher.refresh()).sessions;
}

async function listDesktopOwnerSessionsWithCoverage(): Promise<{
  sessions: DesktopSessionSummary[];
  completeHostIds: string[];
}> {
  await runtimeHostScopeList();
  const catalog = await invokeWhenReady('session-local:catalog') as { scope: DesktopTargetScope; sessions: DesktopSessionSummaryInput[]; authoritative: boolean }[];
  return {
    sessions: catalog.flatMap(({ scope, sessions }) => sessions.map((session) => projectSessionSummary(scope, session))),
    completeHostIds: catalog.filter((entry) => entry.authoritative).map((entry) => entry.scope.hostId),
  };
}

async function listGuestSessionMountCatalog(): Promise<DesktopSessionSummary[]> {
  const mounts: unknown = await invokeWhenReady('session-collaboration:mount:list');
  if (!Array.isArray(mounts)) {
    throw new Error('Desktop shared Session mounts are unavailable');
  }
  const sessions: DesktopSessionSummary[] = [];
  for (const mount of mounts) {
    if (
      !mount ||
      typeof mount !== 'object' ||
      !('mountId' in mount) ||
      typeof mount.mountId !== 'string' ||
      !('name' in mount) ||
      typeof mount.name !== 'string' ||
      !('hostId' in mount) ||
      typeof mount.hostId !== 'string'
    ) {
      throw new Error('Desktop shared Session mount is invalid');
    }
    if (!('session' in mount) || mount.session === undefined) continue;
    const session = decodeSharedSessionCatalogProjection(mount.session);
    const summary = projectDesktopSharedSessionSummary(session);
    const projected = projectDesktopSessionSummary(
      {
        hostId: mount.hostId,
        profileId: mount.mountId,
        profileName: mount.name,
        profileKind: 'remote',
      },
      summary,
    );
    sessions.push(projected);
  }
  return sessions;
}

async function createDesktopSessionOnScope(
  scope: DesktopTargetScope,
  input?: CreateSessionRequestInput,
): Promise<DesktopSessionSummary> {
  const session = await invokeWhenReady('sessions:create', scope, input) as DesktopSessionSummaryInput;
  return projectCreatedSessionSummary(scope, session);
}

function sendActiveRuntimeHost(channel: string, ...args: unknown[]): void {
  void activeRuntimeHostRef()
    .then((scope) => sendWhenReady(channel, scope, ...args))
    .catch(() => undefined);
}

function subscribeActiveRuntimeHostEvent<T extends readonly unknown[]>(
  channel: string,
  handler: (...args: T) => void,
): () => void {
  const listener = (
    _event: Electron.IpcRendererEvent,
    scope: unknown,
    ...args: unknown[]
  ): void => {
    let host: DesktopTargetScope;
    try {
      host = requireDesktopTargetScope(scope);
    } catch {
      return;
    }
    if (
      !activeRuntimeHost ||
      host.hostId !== activeRuntimeHost.hostId ||
      host.targetEpoch !== activeRuntimeHost.targetEpoch
    ) return;
    handler(...(args as unknown as T));
  };
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.off(channel, listener);
}

function subscribeSelectedRuntimeHostEvent<T extends readonly unknown[]>(
  channel: string,
  host: DesktopRuntimeHostRef | undefined,
  handler: (...args: T) => void,
): () => void {
  if (!host) return subscribeActiveRuntimeHostEvent(channel, handler);
  return subscribeEveryRuntimeHostEvent(channel, (scope, ...args: T) => {
    if (
      scope.hostId !== host.hostId ||
      runtimeHostProfiles.get(host.profileId) !== runtimeHostScopeKey(scope)
    ) return;
    handler(...args);
  });
}

const runtimeHost: MakaBridge['runtimeHost'] = {
  query(operation, input) {
    return invokeActiveRuntimeHost('runtime-host:query', operation, input) as Promise<
      OperationOutput<typeof operation>
    >;
  },
  command(operation, input) {
    return invokeActiveRuntimeHost('runtime-host:command', operation, input) as Promise<
      OperationOutput<typeof operation>
    >;
  },
};

async function loadSessionTracePage(
  sessionId: string,
  cursor?: string,
): Promise<DesktopSessionTracePage> {
  const session = await runtimeHostSessionRef(sessionId);
  const host = scopedRuntimeHost(session.scope);
  const page = await host.query(
    'execution.inspect.query',
    cursor
      ? {
          kind: 'session_trace_continue',
          sessionId: session.sessionId,
          cursor,
        }
      : { kind: 'session_trace_start', sessionId: session.sessionId },
  );
  if (page.kind !== 'session_trace_page') throw new Error('Invalid Session trace page');
  const trace = {
    schemaVersion: page.schemaVersion,
    sessionId,
    turns: [...page.turns],
    coverage: page.coverage,
  };
  if (!isSessionTrace(trace)) throw new Error('Invalid Session trace projection');
  return { trace, nextCursor: page.nextCursor };
}

async function loadSessionUsageSummary(
  sessionId: string,
): Promise<Result<DesktopSessionUsageSummary>> {
  const session = await runtimeHostSessionRef(sessionId);
  return invokeWhenReady(
    'usage:summary',
    session.scope,
    { range: 'all', sessionId: session.sessionId },
  ) as Promise<Result<DesktopSessionUsageSummary>>;
}

async function updateDailyReviewConfig(
  patch: Partial<DailyReviewConfig>,
  target?: DesktopRuntimeHostRef,
): Promise<DailyReviewConfig> {
  const host = scopedRuntimeHost(await selectedRuntimeHostScope(target));
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const current = await host.query('daily-review.query', { kind: 'config' });
    if (current.kind !== 'config') throw new Error('Invalid Daily Review config');
    const config = normalizeDailyReviewConfig({ ...current.config, ...patch });
    const result = await host.command('daily-review.mutate', {
      kind: 'update_config',
      expectedRevision: current.revision,
      config,
    });
    if (result.kind === 'config_committed' || result.kind === 'config_unchanged') {
      return result.config;
    }
  }
  throw new Error('Daily Review config kept changing while Desktop updated it');
}

async function listDailyReviewArchives(): Promise<DailyReviewArchiveSummary[]> {
  const host = scopedRuntimeHost(await activeRuntimeHostRef());
  const archives: DailyReviewArchiveSummary[] = [];
  let beforeArchiveId: string | null = null;
  do {
    const result: OperationOutput<'daily-review.query'> = await host.query(
      'daily-review.query', {
      kind: 'archives',
      beforeArchiveId,
      limit: 32,
      },
    );
    if (result.kind !== 'archives') throw new Error('Invalid Daily Review archive page');
    archives.push(...result.archives);
    beforeArchiveId = result.nextBeforeArchiveId;
  } while (beforeArchiveId !== null);
  return archives;
}

function integer(value: unknown, fallback: number): number {
  return Number.isFinite(value) ? Math.trunc(value as number) : fallback;
}

async function bridgeResult<T>(operation: () => Promise<T>, code: string): Promise<Result<T>> {
  try {
    return { ok: true, data: await operation() };
  } catch (error) {
    return {
      ok: false,
      error: {
        code,
        message: error instanceof Error ? error.message : String(error),
      },
    };
  }
}

const browserDocumentId = crypto.randomUUID();
sendWhenReady('browser:document-ready', browserDocumentId);
const browserSelection = createBrowserSelectionCoordinator(runtimeHostSessionRef, {
  capturePage(session) {
    return invokeWhenReady('browser:capture-page', session.scope, session.sessionId);
  },
  show(documentId, generation, session) {
    sendWhenReady(
      'browser:active-session',
      session.scope,
      session.sessionId,
      documentId,
      generation,
    );
  },
  hide(documentId, generation) {
    sendWhenReady('browser:hide-active-session', documentId, generation);
  },
  setViewport(documentId, generation, session, rect) {
    sendWhenReady(
      'browser:setViewport',
      session.scope,
      { sessionId: session.sessionId, rect },
      documentId,
      generation,
    );
  },
}, browserDocumentId);

const makaBridge = {
  workHubControl: workHubControlBridge,
  workHubPresentation: workHubPresentationBridge,
  clientPlugins: {
    subscribeEvents(host, connectionEpoch, request, listener, onError) {
      return subscribeClientEvents(async () => {
        const scope = await runtimeHostScope(host);
        const connection = await invokeWhenReady('plugins:connection', scope, browserDocumentId) as { epoch: string };
        if (connection.epoch !== connectionEpoch) throw new Error('Client connection has retired');
        const current = () => {
          const latest = runtimeHostScopes.get(runtimeHostScopeKey(scope));
          return latest?.hostId === scope.hostId && latest.targetEpoch === scope.targetEpoch;
        };
        return {
          current,
          onRetire: (retire) => {
            const changed = () => { if (!current()) retire(); };
            ipcRenderer.on('runtime-host-profiles:changed', changed);
            return () => ipcRenderer.off('runtime-host-profiles:changed', changed);
          },
          subscribe: (channel, handler) => subscribeRuntimeHostEvent(channel, scope, handler),
          observe: (sessionId, observerId) => invokeWhenReady('sessions:observe', scope, sessionId, observerId),
          unobserve: (observerId) => invokeWhenReady('sessions:unobserve', observerId),
        };
      }, request, listener, onError);
    },
    async authorization(host, connectionEpoch, input, registerCancellation) {
      const requestId = crypto.randomUUID();
      let cancelled = false;
      let cancelPending: (() => void) | undefined;
      registerCancellation(() => { cancelled = true; cancelPending?.(); });
      const scope = await runtimeHostScope(host);
      if (cancelled) throw new Error('Client plugin retired before authorization');
      const pending = invokeWhenReady('plugins:authorization', scope, browserDocumentId, connectionEpoch, input, requestId);
      cancelPending = () => { void invokeWhenReady('plugins:authorization-cancel', scope, browserDocumentId, connectionEpoch, requestId).catch(() => {}); };
      try { return await pending; } finally { cancelPending = undefined; }
    },
    subscribeContext(host, handler) {
      const disposers = [
        subscribeSelectedRuntimeHostEvent('connections:event', host, handler),
        subscribeSelectedRuntimeHostEvent('mcp:changed', host, handler),
        subscribeSelectedRuntimeHostEvent('projects:changed', host, handler),
        subscribeSelectedRuntimeHostEvent<[SessionChangedEvent]>('sessions:changed', host, (event) => {
          if (['updated', 'mode-change', 'turn-status-change', 'rebound'].includes(event.reason)) handler();
        }),
      ];
      return () => { for (const dispose of disposers) dispose(); };
    },
    async file(host, connectionEpoch, identity, input) {
      const scope = await runtimeHostScope(host);
      return invokeWhenReady('plugins:files', scope, browserDocumentId, connectionEpoch, identity, input);
    },
    async connection(host) {
      const scope = await runtimeHostScope(host);
      const connection = await invokeWhenReady('plugins:connection', scope, browserDocumentId) as { epoch: string; hostEpoch: string };
      return { ...connection, localFiles: runtimeHostMetadataFor(scope)?.profileKind === 'local' };
    },
    async session(host, connectionEpoch, sessionId) {
      const scope = await runtimeHostScope(host);
      const connection = await invokeWhenReady('plugins:connection', scope, browserDocumentId) as { epoch: string };
      if (connection.epoch !== connectionEpoch) throw new Error('Client connection has retired');
      return recordRuntimeHostSessionScope(scope, sessionId);
    },
    async remote(host, connectionEpoch, input) {
      const scope = await runtimeHostScope(host);
      // Public plugin bindings are canonical IDs on their originating Host,
      // never Desktop projection keys. The selected connection fixes the Host.
      const value = HOST_OPERATION_SPECS['plugin.remote'].decodeInput(input);
      const result = await invokeWhenReady('plugins:remote', scope, browserDocumentId, connectionEpoch, value);
      if (result?.kind === 'connection_retired') return { kind: 'connection_retired' };
      if (result?.kind === 'remote_error' && typeof result.message === 'string' &&
          HOST_OPERATION_SPECS['plugin.remote'].errors.includes(result.code)) return result;
      return HOST_OPERATION_SPECS['plugin.remote'].decodeOutput(result);
    },
    async query(host, input) {
      const scope = await runtimeHostScope(host);
      return scopedRuntimeHost(scope).query('plugin.client.query', input);
    },
    subscribeChanges(host, handler) {
      return subscribeSelectedRuntimeHostEvent<[string]>('plugins:client-changed', host, handler);
    },
  },
  runtimeHost,
  sessionCollaboration: {
    async prepareInvitation(sessionId, preset, allowInsecure = false) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:prepare',
        session.scope,
        session.sessionId,
        preset,
        allowInsecure,
      );
    },
    async getAccess(sessionId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:getAccess',
        session.scope,
        session.sessionId,
      );
    },
    async revokePrincipal(sessionId, principalId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:revokePrincipal',
        session.scope,
        principalId,
      );
    },
    async renamePrincipal(sessionId, principalId, displayName) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady('session-collaboration:renamePrincipal', session.scope, principalId, displayName);
    },
    async revokeGrant(sessionId, grantId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:revokeGrant',
        session.scope,
        grantId,
      );
    },
    async importInvitation({ code, allowInsecure = false, operationId }, onProgress) {
      const listener = (
        _event: Electron.IpcRendererEvent,
        progressOperationId: string,
        phase: Parameters<NonNullable<typeof onProgress>>[0],
      ) => {
        if (progressOperationId === operationId) onProgress?.(phase);
      };
      ipcRenderer.on('session-collaboration:import:progress', listener);
      try {
        return await invokeWhenReady(
          'session-collaboration:import',
          code,
          allowInsecure,
          operationId,
        );
      } finally {
        ipcRenderer.off('session-collaboration:import:progress', listener);
      }
    },
    cancelImport(operationId) {
      return invokeWhenReady('session-collaboration:import:cancel', operationId);
    },
    readInvitationClipboard() {
      return invokeWhenReady('session-collaboration:invitation:read-clipboard');
    },
    listMounts() {
      return invokeWhenReady('session-collaboration:mount:list');
    },
    subscribeMountChanges(handler) {
      return subscribeGuestSessionMountChanges(() => handler());
    },
    removeMount(mountId) {
      return invokeWhenReady('session-collaboration:mount:remove', mountId);
    },
    retryMount(mountId) {
      return invokeWhenReady('session-collaboration:mount:retry', mountId);
    },
    renameMount(mountId, name) {
      return invokeWhenReady('session-collaboration:mount:rename', mountId, name);
    },
    async requestTurn(sessionId, input) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:turn-request:create',
        session.scope,
        input.kind === 'start'
          ? {
              sessionId: session.sessionId,
              turnId: input.turnId,
              content: { text: input.text },
            }
          : {
              sessionId: session.sessionId,
              turnId: input.turnId,
              sourceTurnId: input.sourceTurnId,
            },
      );
    },
    async getTurnRequests(sessionId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:turn-request:query',
        session.scope,
        session.sessionId,
      );
    },
    async getPendingTurnRequests() {
      const scopes = await readyOwnerRuntimeHostScopes();
      return collectAvailablePendingTurnRequests(
        scopes.map(async (scope) => {
          const result = await invokeWhenReady(
            'session-collaboration:turn-request:query',
            scope,
          ) as CollaborationTurnRequestQueryResult;
          return result.requests
            .filter((request) => request.state.kind === 'pending')
            .map((request): SessionTurnAccessRequest => ({
              ...request,
              intent: {
                ...request.intent,
                sessionId: recordRuntimeHostSessionScope(scope, request.intent.sessionId),
              },
            }));
        }),
      );
    },
    async acknowledgeTurnRequest(sessionId, requestId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:turn-request:acknowledge',
        session.scope,
        requestId,
      );
    },
    async withdrawTurnRequest(sessionId, requestId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:turn-request:withdraw',
        session.scope,
        requestId,
      ) as Promise<CollaborationTurnRequestWithdrawResult>;
    },
    async decideTurnRequest(sessionId, requestId, decision) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady(
        'session-collaboration:turn-request:decide',
        session.scope,
        requestId,
        decision,
      );
    },
  },
  runtimeHostProfiles: {
    getSnapshot() {
      return invokeWhenReady('runtime-host-profiles:getSnapshot');
    },
    async getDefaultHost(): Promise<DesktopRuntimeHostRef> {
      const scope = await activeRuntimeHostRef();
      const metadata = runtimeHostMetadataFor(scope);
      if (!metadata) throw new Error('The default Runtime Host identity is unavailable');
      return { profileId: metadata.profileId, hostId: scope.hostId };
    },
    addAndEnable(input: DesktopRuntimeHostProfileAddInput) {
      return invokeWhenReady('runtime-host-profiles:add-and-enable', input);
    },
    importConnectionCode(code: string) {
      return invokeWhenReady('runtime-host-profiles:import-connection-code', code);
    },
    remove(profileId: string) {
      return invokeWhenReady('runtime-host-profiles:remove', profileId);
    },
    discardPairing(profileId: string) {
      return invokeWhenReady('runtime-host-profiles:discard-pairing', profileId);
    },
    setEnabled(profileId: string, enabled: boolean) {
      return invokeWhenReady('runtime-host-profiles:set-enabled', profileId, enabled);
    },
    setDefault(profileId: string) {
      return invokeWhenReady('runtime-host-profiles:set-default', profileId);
    },
    resolvePairingRecovery(profileId?: string) {
      return invokeWhenReady('runtime-host-profiles:resolve-pairing-recovery', profileId);
    },
    subscribeChanges(handler: (event: DesktopRuntimeHostProfileChangedEvent) => void) {
      const listener = (
        _event: Electron.IpcRendererEvent,
        payload: RuntimeHostProfileWireEvent,
      ) => {
        handler(payload);
      };
      ipcRenderer.on('runtime-host-profiles:changed', listener);
      return () => ipcRenderer.off('runtime-host-profiles:changed', listener);
    },
  },
  runtimeHostHandoff: {
    current: () => invokeWhenReady('runtime-host-handoff:current'),
    decide: (revision, action) => invokeWhenReady('runtime-host-handoff:decide', { revision, action }),
    subscribe(handler) {
      const listener = (_event: Electron.IpcRendererEvent, payload: import('./bridge-contract.js').DesktopHostHandoffPayload | null) => handler(payload);
      ipcRenderer.on('runtime-host-handoff:changed', listener);
      return () => ipcRenderer.off('runtime-host-handoff:changed', listener);
    },
  },
  localRuntimeHostRemoteAccess: {
    getSnapshot() {
      return invokeWhenReady('local-runtime-host-remote-access:get-snapshot');
    },
    enable(input: {
      readonly allowInterruptActiveTasks: boolean;
      readonly coordinationRelays: readonly string[];
    }) {
      return invokeWhenReady('local-runtime-host-remote-access:enable', input);
    },
    createConnectionCode() {
      return invokeWhenReady(
        'local-runtime-host-remote-access:create-connection-code',
      );
    },
    revokeSharedAccess() {
      return invokeWhenReady('local-runtime-host-remote-access:revoke-shared-access');
    },
    disable() {
      return invokeWhenReady('local-runtime-host-remote-access:disable');
    },
  },
  runtimeHostSshTerminal: {
    getSnapshot(): Promise<DesktopRuntimeHostSshTerminalSnapshot> {
      return invokeWhenReady('runtime-host-ssh-terminal:getSnapshot');
    },
    write(sessionId: string, data: string) {
      return invokeWhenReady('runtime-host-ssh-terminal:write', {
        sessionId,
        data,
      });
    },
    resize(sessionId: string, cols: number, rows: number) {
      return invokeWhenReady('runtime-host-ssh-terminal:resize', {
        sessionId,
        cols,
        rows,
      });
    },
    cancel(sessionId: string) {
      return invokeWhenReady('runtime-host-ssh-terminal:cancel', sessionId);
    },
    subscribe(handler: (event: DesktopRuntimeHostSshTerminalEvent) => void) {
      const listener = (
        _event: Electron.IpcRendererEvent,
        payload: DesktopRuntimeHostSshTerminalEvent,
      ) => handler(payload);
      ipcRenderer.on('runtime-host-ssh-terminal:event', listener);
      return () => ipcRenderer.off('runtime-host-ssh-terminal:event', listener);
    },
  },
  runtimeHostOnboarding: {
    listWslDistributions(): Promise<readonly string[]> {
      return invokeWhenReady('runtime-host-onboarding:listWslDistributions');
    },
    getSnapshot(): Promise<DesktopRuntimeHostOnboardingSnapshot> {
      return invokeWhenReady('runtime-host-onboarding:getSnapshot');
    },
    start(input: DesktopRuntimeHostOnboardingInput): Promise<DesktopRuntimeHostOnboardingSnapshot> {
      return invokeWhenReady('runtime-host-onboarding:start', input);
    },
    cancel(): Promise<boolean> {
      return invokeWhenReady('runtime-host-onboarding:cancel');
    },
    reset(): Promise<void> {
      return invokeWhenReady('runtime-host-onboarding:reset');
    },
    subscribe(handler: (snapshot: DesktopRuntimeHostOnboardingSnapshot) => void) {
      const listener = (
        _event: Electron.IpcRendererEvent,
        snapshot: DesktopRuntimeHostOnboardingSnapshot,
      ) => handler(snapshot);
      ipcRenderer.on('runtime-host-onboarding:changed', listener);
      return () => ipcRenderer.off('runtime-host-onboarding:changed', listener);
    },
  },
  runtimeHostManagement: {
    onNativeProgress(profileId, handler) {
      const listener = (_event: Electron.IpcRendererEvent, owner: string, progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => {
        if (owner === profileId) handler(progress);
      };
      ipcRenderer.on('runtime-host-management:native-progress', listener);
      return () => ipcRenderer.off('runtime-host-management:native-progress', listener);
    },
    runNative(request, profileId = 'local') {
      return invokeWhenReady('runtime-host-management:native', request, profileId);
    },
    run(
      profileId: string,
      action: DesktopRuntimeHostManagementAction,
      allowInterruptActiveTasks = false,
    ): Promise<DesktopRuntimeHostManagementResponse> {
      return invokeWhenReady(
        'runtime-host-management:run',
        profileId,
        action,
        allowInterruptActiveTasks,
      );
    },
    update(
      profileId: string,
      allowInterruptActiveTasks: boolean,
    ): Promise<DesktopRuntimeHostManagementResponse> {
      return invokeWhenReady(
        'runtime-host-management:update',
        profileId,
        allowInterruptActiveTasks,
      );
    },
    configureProjectDirectories(
      profileId: string,
      roots: readonly { readonly label: string; readonly path: string }[],
      expectedConfigFingerprint: string,
      allowInterruptActiveTasks: boolean,
    ): Promise<DesktopRuntimeHostManagementResponse> {
      return invokeWhenReady(
        'runtime-host-management:configure-project-directories',
        profileId,
        roots,
        expectedConfigFingerprint,
        allowInterruptActiveTasks,
      );
    },
    subscribeProgress(handler: (progress: DesktopRuntimeHostManagementProgress) => void) {
      const listener = (
        _event: Electron.IpcRendererEvent,
        progress: DesktopRuntimeHostManagementProgress,
      ) => handler(progress);
      ipcRenderer.on('runtime-host-management:progress', listener);
      return () => ipcRenderer.off('runtime-host-management:progress', listener);
    },
    getUpdatePolicy(profileId: string) {
      return invokeWhenReady('runtime-host-management:get-update-policy', profileId);
    },
    setUpdatePolicy(
      profileId: string,
      policy: import('@maka/runtime-host/operator').RuntimeHostManagedUpdatePolicy,
    ) {
      return invokeWhenReady('runtime-host-management:set-update-policy', profileId, policy);
    },
    reconcileUpdate(profileId: string) {
      return invokeWhenReady('runtime-host-management:reconcile-update', profileId);
    },
    getResources(profileId: string) {
      return invokeWhenReady('runtime-host-management:get-resources', profileId);
    },
    getDirectPeer(profileId: string) {
      return invokeWhenReady('runtime-host-management:get-direct-peer', profileId);
    },
    configureDirectPeer(
      profileId: string,
      enabled: boolean,
      coordinationRelays: readonly string[],
      automaticRelayDiscovery: boolean,
      webRtcStunPolicy?: import('@maka/runtime-host/operator').RuntimeHostWebRtcStunPolicy,
    ) {
      return invokeWhenReady(
        'runtime-host-management:configure-direct-peer',
        profileId,
        enabled,
        coordinationRelays,
        automaticRelayDiscovery,
        webRtcStunPolicy,
      );
    },
    createConnectionCode(profileId: string): Promise<string> {
      return invokeWhenReady('runtime-host-management:create-connection-code', profileId);
    },
    listCredentials(profileId: string): Promise<DesktopRuntimeHostAccessSnapshot> {
      return invokeWhenReady('runtime-host-management:list-credentials', profileId);
    },
    rotateCredential(profileId: string): Promise<DesktopRuntimeHostAccessSnapshot> {
      return invokeWhenReady('runtime-host-management:rotate-credential', profileId);
    },
    revokeCredential(
      profileId: string,
      credentialId: string,
    ): Promise<DesktopRuntimeHostAccessSnapshot> {
      return invokeWhenReady(
        'runtime-host-management:revoke-credential',
        profileId,
        credentialId,
      );
    },
  },
  runtimeHostPeerMesh: {
    getConnectivityPolicy() {
      return invokeWhenReady('runtime-host-peer-mesh:get-connectivity-policy');
    },
    setConnectivityPolicy(
      policy: import('@maka/runtime-host/client').RuntimeHostWebRtcStunPolicy,
    ) {
      return invokeWhenReady('runtime-host-peer-mesh:set-connectivity-policy', policy);
    },
    execute(
      target: import('./bridge-contract.js').DesktopRuntimeHostPeerMeshTarget,
      action: import('./bridge-contract.js').DesktopRuntimeHostPeerMeshAction,
      input: {
        readonly meshId?: string | null;
        readonly peerId?: string;
        readonly invitation?: string;
        readonly displayName?: string | null;
        readonly operationId: string;
      },
    ) {
      return invokeWhenReady(
        'runtime-host-peer-mesh:execute',
        target,
        action,
        input.meshId,
        input.peerId,
        input.invitation,
        input.displayName,
        input.operationId,
      );
    },
    cancel(operationId: string) {
      return invokeWhenReady('runtime-host-peer-mesh:cancel', operationId);
    },
  },
  newTasks: {
    async searchExecutors(target, query) {
      return scopedRuntimeHost(await runtimeHostScope(target)).query('executor.catalog.query', query);
    },
    getCatalog(): Promise<DesktopNewTaskCatalog> {
      return loadNewTaskCatalog();
    },
    subscribeChanges(handler: () => void): () => void {
      newTaskChangeListeners.add(handler);
      const unsubscribes = [
        subscribeEveryRuntimeHostEvent('projects:changed', handler),
        subscribeEveryRuntimeHostEvent('connections:event', handler),
        subscribeEveryRuntimeHostEvent('mcp:changed', handler),
        subscribeEveryRuntimeHostEvent('settings:externalChanged', handler),
      ];
      return () => {
        newTaskChangeListeners.delete(handler);
        for (const unsubscribe of unsubscribes) unsubscribe();
      };
    },
    async addProject(host: DesktopNewTaskHostRef, name?: string) {
      const result = await invokeWhenReady(
        'projects:add',
        await runtimeHostScope(host),
        { select: false, name },
      ) as
        | { ok: true; project: ProjectRecord; path: string }
        | { ok: false; reason: 'cancelled' };
      return result.ok ? { ok: true as const, project: result.project } : result;
    },
    async relinkProject(host: DesktopNewTaskHostRef, projectId: string) {
      return invokeWhenReady(
        'projects:relink',
        await runtimeHostScope(host),
        projectId,
      );
    },
    async getConnections(host: DesktopNewTaskHostRef) {
      return invokeWhenReady(
        'connections:getSnapshot',
        await runtimeHostScope(host),
      );
    },
    async getReadiness(
      target: DesktopNewTaskTarget,
      input?: DesktopTaskSubmissionReadinessRequest,
    ) {
      return invokeWhenReady(
        'taskReadiness:getSnapshot',
        await runtimeHostScope(target),
        input,
      );
    },
    async searchFiles(
      target: DesktopNewTaskTarget,
      query: string,
      options?: { limit?: number },
    ) {
      return invokeWhenReady(
        'workspace:searchFiles',
        await runtimeHostScope(target),
        { query, projectId: target.projectId, ...options },
      );
    },
    async create(
      target: DesktopNewTaskTarget,
      input?: CreateSessionRequestInput,
    ): Promise<DesktopSessionSummary> {
      const scope = await runtimeHostScope(target);
      const session = await invokeWhenReady('session-local:create', scope, {
        ...input,
        projectId: target.projectId,
      }) as DesktopSessionSummaryInput;
      return projectCreatedSessionSummary(scope, session);
    },
  },
  pets: {
    list() {
      return invokeWhenReady('pets:list');
    },
    getSelection() {
      return invokeWhenReady('pets:getSelection');
    },
    select(petId: string | null) {
      return invokeWhenReady('pets:select', petId);
    },
    readSpriteSheet(petId: string) {
      return invokeWhenReady('pets:readSpriteSheet', petId);
    },
    remove(petId: string) {
      return invokeWhenReady('pets:remove', petId);
    },
    importLocalDirectory() {
      return invokeWhenReady('pets:importLocalDirectory');
    },
    subscribeChanges(handler: (event: PetPackChangedEvent) => void): () => void {
      const listener = (_event: Electron.IpcRendererEvent, payload: PetPackChangedEvent) =>
        handler(payload);
      ipcRenderer.on('pets:changed', listener);
      return () => ipcRenderer.off('pets:changed', listener);
    },
  },
  workBoard: {
    list(query) {
      return invokeWhenReady('workBoard:list', query);
    },
    create(item) {
      return invokeWhenReady('workBoard:create', item);
    },
    update(id, patch, options) {
      return invokeWhenReady('workBoard:update', id, patch, options);
    },
    archive(id, options) {
      return invokeWhenReady('workBoard:archive', id, options);
    },
    unarchive(id, options) {
      return invokeWhenReady('workBoard:unarchive', id, options);
    },
    remove(id, options) {
      return invokeWhenReady('workBoard:remove', id, options);
    },
    linkSession(id, link, options) {
      return invokeWhenReady('workBoard:linkSession', id, link, options);
    },
    subscribeChanges(handler: (event: WorkBoardChangedEvent) => void): () => void {
      const listener = (_event: Electron.IpcRendererEvent, payload: WorkBoardChangedEvent) =>
        handler(payload);
      ipcRenderer.on('workBoard:changed', listener);
      return () => ipcRenderer.off('workBoard:changed', listener);
    },
  },
  graphs: {
    async listEpochs(rootSessionId: string): Promise<AgentGraphEpochDirectory> {
      const session = await runtimeHostSessionRef(rootSessionId);
      return invokeWhenReady(
        'graphs:listEpochs', session.scope, session.sessionId,
      ) as Promise<AgentGraphEpochDirectory>;
    },
    async listCurrentEpochs(rootSessionId: string): Promise<AgentGraphEpochDirectory> {
      const session = await runtimeHostSessionRef(rootSessionId);
      return invokeWhenReady(
        'graphs:listCurrentEpochs', session.scope, session.sessionId,
      ) as Promise<AgentGraphEpochDirectory>;
    },
    async getSnapshot(
      rootSessionId: string,
      options?: AgentGraphClientSnapshotOptions & { graphId?: string },
    ): Promise<AgentGraphClientSnapshot> {
      const session = await runtimeHostSessionRef(rootSessionId);
      const snapshot = await invokeWhenReady(
        'graphs:getSnapshot', session.scope, session.sessionId, options,
      ) as AgentGraphClientSnapshot;
      return projectProtocolSessionIds(session.scope.hostId, snapshot);
    },
    async inspectOperator(
      rootSessionId: string,
      operatorId: string,
      graphId?: string,
    ): Promise<AgentGraphOperatorInspection> {
      const session = await runtimeHostSessionRef(rootSessionId);
      const inspection = await invokeWhenReady(
        'graphs:inspectOperator',
        session.scope,
        session.sessionId,
        operatorId,
        graphId,
      ) as AgentGraphOperatorInspection;
      return projectProtocolSessionIds(session.scope.hostId, inspection);
    },
    stop(rootSessionId: string, expectedGraphId: string): Promise<void> {
      return invokeSessionRuntimeHost('graphs:stop', rootSessionId, expectedGraphId);
    },
    subscribe(
      rootSessionId: string,
      handler: () => void,
    ): () => void {
      let disposed = false;
      const unsubscribes: Array<() => void> = [];
      void runtimeHostSessionRef(rootSessionId)
        .then(({ scope, sessionId }) => {
          if (disposed) return;
          const onChanged = (payload: { rootSessionId: string }): void => {
            if (payload.rootSessionId === sessionId) handler();
          };
          unsubscribes.push(
            subscribeRuntimeHostEvent('graphs:changed', scope, onChanged),
            subscribeRuntimeHostEvent('graphs:resync', scope, onChanged),
          );
        })
        .catch(() => undefined);
      return () => {
        disposed = true;
        for (const unsubscribe of unsubscribes) unsubscribe();
      };
    },
  },
  sessionLocal: {
    async listMessages(sessionId) {
      const session = await runtimeHostSessionRef(sessionId);
      const records = await invokeWhenReady('session-local:messages', session.scope, session.sessionId) as import('../shared/session-local-contract.js').DesktopLocalMessage[];
      return records.map((record) => ({ ...record, sessionId, attachments: projectDesktopAttachmentRefs(session.scope, record.attachments) }));
    },
    async cancelMessage(sessionId, messageId) {
      const session = await runtimeHostSessionRef(sessionId);
      await invokeWhenReady('session-local:cancel', session.scope, session.sessionId, messageId);
    },
    async reconcileMessage(sessionId, messageId) {
      const session = await runtimeHostSessionRef(sessionId);
      await invokeWhenReady('session-local:reconcile', session.scope, session.sessionId, messageId);
    },
    async readTranscript(sessionId) {
      const session = await runtimeHostSessionRef(sessionId);
      return invokeWhenReady('session-local:transcript', session.scope, session.sessionId);
    },
    subscribeChanges(handler) {
      return subscribeEveryRuntimeHostEvent('session-local:changed', (scope, event: { sessionId?: string }) => {
        if (event.sessionId) handler(recordRuntimeHostSessionScope(scope, event.sessionId));
      });
    },
  } satisfies import('../shared/session-local-contract.js').DesktopSessionLocalBridge,
  sessions: {
    async queryTurn(sessionId: string, turnId: string) {
      const session = await runtimeHostSessionRef(sessionId);
      const turn = await scopedRuntimeHost(session.scope).query('turn.query', { sessionId: session.sessionId, turnId });
      if (turn.sessionId !== session.sessionId || turn.turnId !== turnId) throw new Error('Turn query identity changed');
      return { ...turn, sessionId };
    },
    async get(sessionId: string) {
      const session = await runtimeHostSessionRef(sessionId);
      const read = desktopSessionCatalogRefresher.beginRowRead(sessionId);
      const summary = await invokeWhenReady('sessions:get', session.scope, session.sessionId) as DesktopSessionSummaryInput | null;
      if (summary && summary.id !== session.sessionId) throw new Error('Session query identity changed');
      const projected = summary && projectSessionSummary(session.scope, summary);
      if (!read.commit(projected)) throw new Error('Session read was superseded');
      return projected;
    },
    list(filter?: SessionListFilter): Promise<DesktopSessionSummary[]> {
      return listDesktopSessions(filter);
    },
    listWithCoverage() {
      return desktopSessionCatalogRefresher.refresh();
    },
    /**
     * The single session-creation channel (#1433). `mode` names a
     * product intent — main derives the permission boundary, name and
     * labels it implies (`create-session-input.ts`); the renderer cannot
     * reach a boundary like `explore` by asking for it directly.
     */
    async create(input?: CreateSessionRequestInput): Promise<DesktopSessionSummary> {
      const scope = await activeRuntimeHostRef();
      return createDesktopSessionOnScope(scope, input);
    },
    async send(sessionId, command) {
      const session = await runtimeHostSessionRef(sessionId);
      if (command.directoryReferences?.some((ref) => ref.hostId !== session.scope.hostId)) {
        throw new Error('Directory references belong to a different Runtime Host. Select the folder on the target Host.');
      }
      let attachmentItems: Awaited<ReturnType<typeof encodeIngestItems>> | undefined;
      try {
        attachmentItems =
          'attachmentItems' in command && command.attachmentItems
            ? await encodeIngestItems(command.attachmentItems)
            : undefined;
      } catch (error) {
        if (error instanceof AttachmentIngestBlockedError) {
          return { ok: false, reason: 'attachment_blocked', code: error.code };
        }
        throw error;
      }
      const encoded = attachmentItems ? { ...command, attachmentItems } : command;
      const result = (await invokeWhenReady(
        'sessions:send',
        session.scope,
        session.sessionId,
        encoded,
      )) as Awaited<ReturnType<MakaBridge['sessions']['send']>>;
      return result.ok
        ? { ...result, attachments: projectDesktopAttachmentRefs(session.scope, result.attachments) }
        : result;
    },
    compact(sessionId: string): Promise<OperationOutput<'context.compact'>> {
      return invokeSessionRuntimeHost('sessions:compact', sessionId);
    },
    resumeLatest(sessionId: string): Promise<
      | { disposition: 'started'; runId: string; turnId: string }
      | { disposition: 'park'; rejectionReasons: string[]; diagnostics: unknown[] }
    > {
      return invokeSessionRuntimeHost('sessions:resumeLatest', sessionId);
    },
    stop(
      sessionId: string,
      input?: {
        source?: 'stop_button';
        expectedTurnId?: string;
        expectedAdmissionId?: string;
      },
    ): Promise<DesktopSessionStopResult> {
      return invokeSessionRuntimeHost('sessions:stop', sessionId, input);
    },
    async submitMessage(sessionId, placement, command, options) {
      const session = await runtimeHostSessionRef(sessionId);
      if (command.directoryReferences?.some((ref) => ref.hostId !== session.scope.hostId)) {
        throw new Error('Directory references belong to a different Runtime Host. Select the folder on the target Host.');
      }
      let attachmentItems: Awaited<ReturnType<typeof encodeIngestItems>> | undefined;
      try {
        attachmentItems = command.attachmentItems
          ? await encodeIngestItems(command.attachmentItems)
          : undefined;
      } catch (error) {
        if (error instanceof AttachmentIngestBlockedError) {
          return { ok: false, reason: 'attachment_blocked', code: error.code };
        }
        throw error;
      }
      const result = (await invokeWhenReady(
        options?.waitForHostAdmission ? 'sessions:submitMessage' : 'session-local:submit',
        session.scope,
        session.sessionId,
        placement,
        {
          ...command,
          ...(command.retainedAttachments ? { retainedAttachments: hostAttachmentRefs(session, command.retainedAttachments) } : {}),
          ...(attachmentItems ? { attachmentItems } : {}),
        },
      )) as Awaited<ReturnType<MakaBridge['sessions']['submitMessage']>>;
      return result.ok
        ? { ...result, attachments: projectDesktopAttachmentRefs(session.scope, result.attachments) }
        : result;
    },
    queryCancelledMessages(sessionId, messageIds) {
      return invokeSessionRuntimeHost('sessions:queryCancelledMessages', sessionId, messageIds);
    },
    queryMessageExecutions(sessionId, messageIds) {
      return invokeSessionRuntimeHost('sessions:queryMessageExecutions', sessionId, messageIds);
    },
    retractQueueEntry(sessionId: string, entryId: string): Promise<void> {
      return invokeSessionRuntimeHost('sessions:retractQueueEntry', sessionId, entryId);
    },
    promoteQueueEntry(sessionId: string, entryId: string): Promise<void> {
      return invokeSessionRuntimeHost('sessions:promoteQueueEntry', sessionId, entryId);
    },
    updateQueueEntry(
      sessionId: string,
      entryId: string,
      expectedQueueRevision: number,
      text: string,
    ): Promise<void> {
      return invokeSessionRuntimeHost(
        'sessions:updateQueueEntry',
        sessionId,
        entryId,
        expectedQueueRevision,
        text,
      );
    },
    reorderQueueEntries(sessionId: string, entryIds: readonly string[]): Promise<void> {
      return invokeSessionRuntimeHost('sessions:reorderQueueEntries', sessionId, [...entryIds]);
    },
    readExecutionBoundary(sessionId: string): Promise<ExecutionBoundaryReadModel> {
      return invokeSessionRuntimeHost('sessions:readExecutionBoundary', sessionId);
    },
    listActiveInteractions(sessionId: string): Promise<ActiveInteractionRequestEvent[]> {
      return invokeSessionRuntimeHost('sessions:listActiveInteractions', sessionId);
    },
    subscribeActiveInteractions(
      handler: (event: {
        sessionId: string;
        interactions: ActiveInteractionRequestEvent[];
      }) => void,
    ): () => void {
      return subscribeEveryRuntimeHostEvent(
        'sessions:active-interactions-changed',
        (scope, event: { sessionId: string; interactions: ActiveInteractionRequestEvent[] }) =>
          handler({
            ...event,
            sessionId: recordRuntimeHostSessionScope(scope, event.sessionId),
          }),
      );
    },
    async listTurns(sessionId: string): Promise<TurnRecord[]> {
      const session = await runtimeHostSessionRef(sessionId);
      const turns = await invokeWhenReady(
        'sessions:listTurns',
        session.scope,
        session.sessionId,
      ) as TurnRecord[];
      return turns.map((turn) => projectDesktopTurnRecord(session.scope, turn));
    },
    readQuote(sessionId) {
      return invokeSessionRuntimeHost('sessions:readQuote', sessionId);
    },
    listTurnLandmarks(sessionId, turnId = null) {
      return invokeProjectedSessionRuntimeHost('sessions:listTurnLandmarks', sessionId, turnId);
    },
    regenerateTurn(sessionId: string, input: RegenerateTurnInput): Promise<void> {
      return invokeSessionRuntimeHost('sessions:regenerateTurn', sessionId, input);
    },
    branchFromTurn: invokeBranchFromTurn,
    async reviseBeforeTurn(sessionId: string, input: DesktopReviseBeforeTurnInput): Promise<DesktopSessionSummary> {
      const ref = await runtimeHostSessionRef(sessionId);
      const summary = await invokeWhenReady(
        'sessions:reviseBeforeTurn', ref.scope, ref.sessionId, input,
      ) as DesktopSessionSummaryInput;
      return projectCreatedSessionSummary(ref.scope, summary);
    },
    respondToSandboxBoundary(sessionId: string, response: SandboxBoundaryResponse): Promise<void> {
      return invokeSessionRuntimeHost('sessions:respondToSandboxBoundary', sessionId, response);
    },
    respondToClientCapability(
      sessionId: string,
      response: ClientCapabilityResponse,
    ): Promise<void> {
      return invokeSessionRuntimeHost(
        'sessions:respondToClientCapability',
        sessionId,
        response,
      );
    },
    respondToUserQuestion(sessionId: string, response: UserQuestionResponse): Promise<void> {
      return invokeSessionRuntimeHost('sessions:respondToUserQuestion', sessionId, response);
    },
    respondToUserForm(sessionId: string, response: InteractionFormResponse): Promise<void> {
      return invokeSessionRuntimeHost('sessions:respondToUserForm', sessionId, response);
    },
    respondToPermissions(sessionId: string, response: import('@maka/core/execution-permissions').PermissionsResponse): Promise<void> {
      return invokeSessionRuntimeHost('sessions:respondToPermissions', sessionId, response);
    },
    /**
     * PR-CMD-PALETTE-SAVE-CONVERSATION-FILE-0: write the renderer-formatted
     * conversation markdown to a user-chosen file. Renderer owns the
     * `renderConversationMarkdown` step (it knows the session name + raw
     * message stream); main owns the save dialog + file write.
     */
    saveConversationToFile(input: {
      markdown: string;
      defaultName: string;
    }): Promise<
      { ok: true; path: string } | { ok: false; reason: 'canceled' | 'write_failed' | 'invalid_input' }
    > {
      return invokeWhenReady('chat:saveConversationToFile', input);
    },
    subscribeEvents(
      sessionId: string,
      handler: (event: SessionEvent) => void,
      onObservationSeed?: (phase: 'pending' | 'ready') => void,
      onSeedError?: (error: unknown) => void,
      onExecution?: (projection: import('../shared/session-execution-projection.js').SessionExecutionProjection | undefined) => void,
    ): () => void {
      const observerId = crypto.randomUUID();
      let lastExecution: import('../shared/session-execution-projection.js').SessionExecutionProjection | undefined;
      let disposed = false;
      let unsubscribeEvents = () => {};
      const acceptExecution = (projection: import('../shared/session-execution-projection.js').SessionExecutionProjection) => {
        lastExecution = { ...projection, rootTurn: projection.rootTurn ? { ...projection.rootTurn, sessionId } : null };
        onExecution?.(lastExecution);
      };
      const observeDispatch = runtimeHostSessionRef(sessionId).then((session) => {
        if (disposed) {
          return {
            completion: Promise.resolve({ kind: 'cancelled' } as const),
          };
        }
        const profileId = runtimeHostMetadataFor(session.scope)?.profileId;
        if (!profileId) throw new Error('The Runtime Host profile for this task is unavailable');
        // Keep the renderer listener across Host target epochs. The observer
        // registry restores this observer on the replacement target. Profile
        // identity admits that replacement without accepting another Host's
        // same-named Session channel.
        unsubscribeEvents = subscribeEveryRuntimeHostEvent(
          `sessions:event:${session.sessionId}`,
          (scope, event: SessionEvent | SessionObservationMessage) => {
            if (disposed) return;
            if (runtimeHostMetadataFor(scope)?.profileId !== profileId) return;
            if (event.type === 'host_observation_seed') {
              if (!event.observerIds.includes(observerId)) return;
              // Seed and live updates share this ordered channel. Readiness
              // follows consumption, never the separate registration reply.
              acceptExecution(event.execution);
              for (const seededEvent of event.events) {
                if (disposed) return;
                handler(projectDesktopSessionEvent(scope, seededEvent));
              }
              if (!disposed) onObservationSeed?.('ready');
              return;
            }
            if (event.type === 'host_observation_pending') {
              if (lastExecution) lastExecution = { ...lastExecution, available: false };
              onExecution?.(lastExecution);
              onObservationSeed?.('pending');
              return;
            }
            if (event.type === 'host_execution') {
              acceptExecution(event);
              return;
            }
            if (event.type === 'host_observation_error') {
              onSeedError?.(new Error(event.message));
              return;
            }
            handler(projectDesktopSessionEvent(scope, event));
          },
        );
        return {
          completion: invokeWhenReady(
            'sessions:observe',
            session.scope,
            session.sessionId,
            observerId,
          ) as Promise<RuntimeHostObservationIpcResult<void>>,
        };
      });
      const observing = observeDispatch.then(({ completion }) => completion);
      void observing.then(
        (result) => {
          if (result.kind === 'cancelled') {
            disposed = true;
            unsubscribeEvents();
          }
        },
        (error: unknown) => {
          if (!disposed) onSeedError?.(error);
        },
      );
      return () => {
        disposed = true;
        unsubscribeEvents();
        void releaseSessionObservation(observeDispatch, () =>
          invokeWhenReady('sessions:unobserve', observerId),
        ).catch(() => undefined);
      };
    },
    subscribeChanges(handler: (event: SessionChangedEvent) => void): () => void {
      const unsubscribeRuntimeHosts = subscribeEveryRuntimeHostEvent(
        'sessions:changed',
        (scope, event: SessionChangedEvent) => {
          if (!event.sessionId) {
            handler(event);
            return;
          }
          const observed = observeRuntimeHostSessionScope(scope, event.sessionId);
          handler(observed.authorityAccepted
            ? { ...event, sessionId: observed.sessionId }
            : { reason: 'updated', ts: event.ts });
        },
      );
      const unsubscribeMounts = subscribeGuestSessionMountChanges(() => {
        handler({ reason: 'updated', ts: Date.now() });
      });
      return () => {
        unsubscribeRuntimeHosts();
        unsubscribeMounts();
      };
    },
    archive(sessionId: string, options?: { revisionFamily?: boolean }): Promise<void> {
      return invokeSessionRuntimeHost('sessions:archive', sessionId, options);
    },
    unarchive(sessionId: string, options?: { revisionFamily?: boolean }): Promise<void> {
      return invokeSessionRuntimeHost('sessions:unarchive', sessionId, options);
    },
    setFlagged(sessionId: string, isFlagged: boolean, options?: { revisionFamily?: boolean }): Promise<void> {
      return invokeSessionRuntimeHost('sessions:setFlagged', sessionId, isFlagged, options);
    },
    rename(sessionId: string, name: string, options?: { revisionFamily?: boolean }): Promise<void> {
      return invokeSessionRuntimeHost('sessions:rename', sessionId, name, options);
    },
    setSandboxMode(sessionId: string, mode: SandboxMode): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setSandboxMode', sessionId, mode);
    },
    moveToProject(sessionId: string, projectId: string | null): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:moveToProject', sessionId, projectId);
    },
    setApprovalPolicy(sessionId: string, policy: import('@maka/core/execution-permissions').ApprovalPolicy): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setApprovalPolicy', sessionId, policy);
    },
    setExecutionPolicy(sessionId: string, policy: import('@maka/core/execution-permissions').ExecutionPolicy): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setExecutionPolicy', sessionId, policy);
    },
    setCollaborationMode(sessionId: string, mode: CollaborationMode): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setCollaborationMode', sessionId, mode);
    },
    setOrchestrationMode(sessionId: string, mode: OrchestrationMode): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setOrchestrationMode', sessionId, mode);
    },
    getPlanState(sessionId: string): Promise<PlanSessionState> {
      return invokeProjectedSessionRuntimeHost('plan-mode:getState', sessionId);
    },
    subscribePlanChanges(sessionId: string, handler: () => void): () => void {
      let disposed = false;
      let unsubscribe = () => {};
      void runtimeHostSessionRef(sessionId)
        .then((session) => {
          if (disposed) return;
          unsubscribe = subscribeRuntimeHostEvent(
            'plan-mode:changed',
            session.scope,
            (payload: { sessionId: string }) => {
              if (payload.sessionId === session.sessionId) handler();
            },
          );
        })
        .catch(() => undefined);
      return () => {
        disposed = true;
        unsubscribe();
      };
    },
    requestPlanRevision(sessionId: string, proposalId: string): Promise<PlanControlIpcResult<PlanSessionState>> {
      return invokeProjectedSessionRuntimeHost('plan-mode:requestRevision', sessionId, proposalId);
    },
    abandonPlanProposal(
      sessionId: string,
      proposalId: string,
    ): Promise<PlanControlIpcResult<PlanSessionState>> {
      return invokeProjectedSessionRuntimeHost('plan-mode:abandon', sessionId, proposalId);
    },
    approvePlan(sessionId: string, input: {
      proposalId: string;
      expectedRevision: number;
      expectedStoreVersion: number;
      turnId: string;
    }): Promise<PlanControlIpcResult<{ turnId: string; executionId: string }>> {
      return invokeSessionRuntimeHost('plan-mode:approve', sessionId, input);
    },
    resumePlan(sessionId: string, executionId: string, turnId: string): Promise<PlanControlIpcResult<{
      turnId: string;
      executionId: string;
    }>> {
      return invokeSessionRuntimeHost('plan-mode:resume', sessionId, executionId, turnId);
    },
    abandonPlanExecution(sessionId: string, executionId: string): Promise<PlanControlIpcResult<PlanSessionState>> {
      return invokeProjectedSessionRuntimeHost('plan-mode:abandonExecution', sessionId, executionId);
    },
    async searchExecutors(sessionId, query) {
      const session = await runtimeHostSessionRef(sessionId);
      return scopedRuntimeHost(session.scope).query('executor.catalog.query', { ...query, scope: `session:${session.sessionId}` });
    },
    setExecutorConfiguration(sessionId, input) {
      return invokeSessionUpdate('sessions:setExecutorConfiguration', sessionId, input);
    },
    setModelConfiguration(sessionId: string, input: {
      llmConnectionId: string;
      llmConnectionSlug: string;
      model: string;
      thinkingLevel: ThinkingLevel | null;
    }): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setModelConfiguration', sessionId, input);
    },
    setThinkingLevel(sessionId: string, level: ThinkingLevel | undefined | null): Promise<DesktopSessionUpdateResult<DesktopSessionSummary>> {
      return invokeSessionUpdate('sessions:setThinkingLevel', sessionId, level ?? undefined);
    },
    async remove(
      sessionId: string,
      options?: { revisionFamily?: boolean; requireArchived?: boolean },
    ): Promise<{ disposition: 'removed' | 'restored'; archivedSubtaskCount: number }> {
      const session = await runtimeHostSessionRef(sessionId);
      if (await invokeWhenReady('session-local:discard', session.scope, session.sessionId)) {
        return { disposition: 'removed', archivedSubtaskCount: 0 };
      }
      return invokeSessionRuntimeHost('sessions:remove', sessionId, options);
    },
    previewRemoval(sessionId: string): Promise<number> {
      return invokeSessionRuntimeHost('sessions:removePreview', sessionId);
    },
    cleanupSessionCopy(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('sessions:cleanupSessionCopy', sessionId);
    },
    async abandonSessionCopy(sourceSessionId: string, copyId: string): Promise<void> {
      const source = await runtimeHostSessionRef(sourceSessionId);
      await invokeWhenReady('sessions:abandonSessionCopy', source.scope, copyId);
    },
  },
  transcripts: {
    async readTurn(sessionId: string, turnId: string): Promise<StoredMessage[]> {
      const session = await runtimeHostSessionRef(sessionId);
      const messages = await invokeWhenReady(
        'sessions:transcript:read-turn',
        session.scope,
        session.sessionId,
        turnId,
      ) as StoredMessage[];
      return messages.map((message) => projectDesktopStoredMessage(session.scope, message));
    },
    async open(
      sessionId: string,
      handler: (batch: DesktopTranscriptBatch) => void,
      registerCancellation?: (cancel: () => void) => void,
      mode: DesktopTranscriptOpenMode = 'history',
      resumeFrom?: number,
    ): Promise<DesktopTranscriptHandle> {
      const consumerId = crypto.randomUUID();
      const channel = `sessions:transcript:${consumerId}`;
      let identity: DesktopTranscriptIdentity | undefined;
      let session: Awaited<ReturnType<typeof runtimeHostSessionRef>> | undefined;
      const retiredGenerations = new Set<string>();
      let closed = false;
      let requestClose = () => {};
      let consumerScope: DesktopTargetScope | undefined;
      const listener = (
        _event: Electron.IpcRendererEvent,
        scope: unknown,
        value: unknown,
      ) => {
        if (closed) return;
        let batch: DesktopTranscriptBatch;
        try {
          const host = requireDesktopTargetScope(scope);
          if (
            !consumerScope ||
            host.hostId !== consumerScope.hostId ||
            host.targetEpoch !== consumerScope.targetEpoch
          ) return;
          batch = assertDesktopTranscriptBatch(value);
          if (!retiredGenerations.has(batch.generation)) {
            const adopted = adoptTranscriptIdentity(identity, batch);
            if (adopted !== identity) {
              if (identity && identity.generation !== adopted.generation) retiredGenerations.add(identity.generation);
              identity = adopted;
              consumerScope = host;
            }
            if (identity !== undefined && batch.generation === identity.generation) handler(batch);
          }
        } catch (error) {
          requestClose();
          throw error;
        }
        if (consumerScope) {
          void invokeWhenReady(
            'sessions:transcript:ack',
            consumerScope,
            consumerId,
            batch.generation,
            batch.deliverySequence,
          ).catch(requestClose);
        }
      };
      ipcRenderer.on(channel, listener);
      const openDispatch = runtimeHostSessionRef(sessionId).then((ref) => {
        consumerScope = ref.scope;
        session = ref;
        if (closed) throw new Error('Desktop transcript open was cancelled');
        return {
          completion: invokeWhenReady(
            'sessions:transcript:open',
            ref.scope,
            ref.sessionId,
            consumerId,
            mode,
            resumeFrom ?? null,
          ) as Promise<RuntimeHostObservationIpcResult<DesktopTranscriptOpenResult>>,
        };
      });
      let closeTask: Promise<void> | undefined;
      requestClose = () => {
        if (closed) return;
        closed = true;
        ipcRenderer.off(channel, listener);
        closeTask = releaseSessionObservation(openDispatch, () =>
          invokeWhenReady('sessions:transcript:close', consumerId),
        );
        void closeTask.catch(() => undefined);
      };
      registerCancellation?.(requestClose);
      let openResult: RuntimeHostObservationIpcResult<DesktopTranscriptOpenResult>;
      try {
        openResult = await openDispatch.then(({ completion }) => completion);
      } catch (error) {
        ipcRenderer.off(channel, listener);
        try {
          // A healthy open publishes only live history. The cache is a read-only
          // fallback, never a preview or a continuation of a partial live read.
          if (!closed && !identity && session) {
            const cached = await invokeWhenReady(
              'session-local:transcript', session.scope, session.sessionId,
            ).catch(() => null) as import('../shared/session-local-contract.js').DesktopCachedTranscript | null;
            let cachedIdentity: DesktopTranscriptIdentity | undefined;
            // Cache delivery is outside the live ACK window. Cancellation must
            // still win while the local read or its consumer is running.
            for (const [index, batch] of (cached?.batches ?? []).entries()) {
              if (closed) throw new Error('Desktop transcript open was cancelled');
              handler({ ...batch, deliverySequence: index + 1 });
              if (batch.ready) cachedIdentity = { generation: batch.generation, hostEpoch: batch.hostEpoch };
            }
            if (closed) throw new Error('Desktop transcript open was cancelled');
            if (cachedIdentity) {
              const unavailable = async () => { throw new Error('Reconnect the Host to load uncached history'); };
              return {
                ...cachedIdentity, sessionId, readThroughMessageId: null,
                acknowledgeTail: unavailable,
                loadEarlier: unavailable,
                close: async () => {},
              };
            }
          }
          throw error;
        } finally {
          closed = true;
        }
      }
      if (openResult.kind === 'cancelled') {
        closed = true;
        ipcRenderer.off(channel, listener);
        throw new Error('Desktop transcript open was cancelled');
      }
      const opened = openResult.value;
      if (closed) throw new Error('Desktop transcript open was cancelled');
      identity ??= { generation: opened.generation, hostEpoch: opened.hostEpoch };
      return {
        ...opened,
        sessionId,
        acknowledgeTail: (through) => {
          const currentIdentity = identity;
          if (!currentIdentity) {
            throw new Error('Desktop transcript identity is unavailable');
          }
          return invokeWhenReady('sessions:transcript:acknowledge-tail', consumerScope, {
            consumerId,
            sessionId: opened.sessionId,
            hostEpoch: currentIdentity.hostEpoch,
            through,
          }) as Promise<void>;
        },
        loadEarlier: (throughSequence) =>
          invokeWhenReady(
            'sessions:transcript:load-earlier',
            consumerScope,
            consumerId,
            throughSequence ?? null,
          ) as Promise<void>,
        async close() {
          if (closed) return;
          requestClose();
          await closeTask;
        },
      };
    },
  },
  externalSessions: {
    listSources(host?: DesktopRuntimeHostRef): Promise<{ adapterIds: string[] }> {
      return invokeSelectedRuntimeHost(host, 'external-sessions:listSources');
    },
    async list(input: {
      adapterId: string;
      includeArchived?: boolean;
      cursor?: string;
      text?: string;
    }, host?: DesktopRuntimeHostRef): Promise<{
      sessions: DesktopExternalSessionCatalogItem[];
      nextCursor: string | null;
    }> {
      const scope = await selectedRuntimeHostScope(host);
      const result = await invokeWhenReady(
        'external-sessions:list', scope, input,
      ) as {
        sessions: DesktopHostExternalSessionCatalogItem[];
        nextCursor: string | null;
      };
      return {
        ...result,
        sessions: result.sessions.map((session) =>
          projectDesktopExternalSessionCatalogItem(scope, session),
        ),
      };
    },
    async import(input: {
      adapterId: string;
      sourceSessionId: string;
    }, host?: DesktopRuntimeHostRef): Promise<ExternalSessionImportIpcResult<DesktopSessionSummary>> {
      const scope = await selectedRuntimeHostScope(host);
      const result = await invokeWhenReady(
        'external-sessions:import', scope, input,
      ) as ExternalSessionImportIpcResult;
      return result.ok
        ? { ...result, session: projectCreatedSessionSummary(scope, result.session as DesktopSessionSummaryInput) }
        : result;
    },
  },
  sessionBundles: {
    // Both halves name a path the Electron picker chose, which is a path on
    // THIS machine, and the protocol interprets it on the Host's filesystem.
    // Those are the same filesystem only for the Local Host, so both are routed
    // there explicitly -- not to whichever Host is active, and not to whichever
    // one Settings happens to be pointed at. Carrying a bundle to or from a
    // remote Host needs a byte transfer, not a path string.
    async export(input: {
      sessionId: string;
      suggestedName: string;
      confirmedSubtree?: readonly string[];
    }): Promise<SessionBundleExportIpcResult> {
      const scope = await localRuntimeHostRef();
      const { sessionId } = parseDesktopSessionKey(input.sessionId);
      // Unprojected here, where the boundary already is: the renderer holds
      // host-scoped ids and the Host knows only its own, so a digest computed
      // upstream would compare two different alphabets and never match.
      const confirmed = input.confirmedSubtree?.map(
        (projected) => parseDesktopSessionKey(projected).sessionId,
      );
      return (await invokeWhenReady(
        'session-bundle:export', scope, sessionId, input.suggestedName, confirmed,
      )) as SessionBundleExportIpcResult;
    },
    async import(): Promise<SessionBundleImportIpcResult> {
      const scope = await localRuntimeHostRef();
      return (await invokeWhenReady('session-bundle:import', scope)) as SessionBundleImportIpcResult;
    },
  },
  projects: {
    async getDefaultContext(host?: DesktopRuntimeHostRef): Promise<{
      snapshot: DesktopProjectSnapshot;
      info: DesktopAppInfo;
    }> {
      const scope = await selectedRuntimeHostScope(host);
      const [snapshot, info] = await Promise.all([
        invokeWhenReady('projects:getSnapshot', scope) as Promise<DesktopProjectSnapshot>,
        invokeWhenReady('app:info', scope) as Promise<DesktopAppInfo>,
      ]);
      return { snapshot, info };
    },
    getSnapshot(sessionId?: string, host?: DesktopRuntimeHostRef): Promise<DesktopProjectSnapshot> {
      return sessionId
        ? invokeSessionRuntimeHost('projects:getSnapshot', sessionId)
        : invokeSelectedRuntimeHost(host, 'projects:getSnapshot');
    },
    subscribeChanges(handler: () => void, sessionId?: string, host?: DesktopRuntimeHostRef): () => void {
      if (!sessionId) return subscribeSelectedRuntimeHostEvent('projects:changed', host, handler);
      let disposed = false;
      let unsubscribe = (): void => {};
      void runtimeHostSessionRef(sessionId).then((session) => {
        if (disposed) return;
        unsubscribe = subscribeRuntimeHostEvent('projects:changed', session.scope, handler);
      }).catch(() => undefined);
      return () => {
        disposed = true;
        unsubscribe();
      };
    },
    async getLocalSnapshot(): Promise<DesktopProjectSnapshot> {
      return invokeWhenReady(
        'projects:getSnapshot',
        await localRuntimeHostRef(),
      ) as Promise<DesktopProjectSnapshot>;
    },
    subscribeLocalChanges(handler: () => void): () => void {
      return subscribeEveryRuntimeHostEvent('projects:changed', (scope) => {
        if (runtimeHostMetadataFor(scope)?.profileKind === 'local') handler();
      });
    },
    add(host?: DesktopRuntimeHostRef, options?: { name?: string }): Promise<
      { ok: true; project: ProjectRecord; path: string } | { ok: false; reason: 'cancelled' }
    > {
      return invokeSelectedRuntimeHost(host, 'projects:add', options);
    },
    getDirectoryRoots(host: DesktopRuntimeHostRef) {
      return invokeSelectedRuntimeHost(host, 'projects:directoryRoots');
    },
    listDirectory(
      input: { readonly rootId: string; readonly segments: readonly string[] },
      host: DesktopRuntimeHostRef,
    ) {
      return invokeSelectedRuntimeHost(host, 'projects:listDirectory', input);
    },
    registerDirectory(
      input: { readonly rootId: string; readonly segments: readonly string[]; readonly name?: string },
      host: DesktopRuntimeHostRef,
    ) {
      return invokeSelectedRuntimeHost(host, 'projects:registerDirectory', input);
    },
    select(
      projectId: string | null,
      host?: DesktopRuntimeHostRef,
    ): Promise<{ project: ProjectRecord | null; path: string }> {
      return invokeSelectedRuntimeHost(host, 'projects:select', projectId);
    },
    relink(projectId: string, host?: DesktopRuntimeHostRef): Promise<
      { ok: true; project: ProjectRecord } | { ok: false; reason: 'cancelled' }
    > {
      return invokeSelectedRuntimeHost(host, 'projects:relink', projectId);
    },
    reveal(projectId: string, host?: DesktopRuntimeHostRef): Promise<
      | { ok: true; opened: string }
      | {
          ok: false;
          reason: 'unknown-key' | 'not-allowed' | 'missing' | 'not-a-directory' | 'open-failed';
        }
    > {
      return invokeSelectedRuntimeHost(host, 'projects:reveal', projectId);
    },
    rename(projectId: string, name: string, host?: DesktopRuntimeHostRef): Promise<ProjectRecord> {
      return invokeSelectedRuntimeHost(host, 'projects:rename', projectId, name);
    },
    archive(projectId: string, host?: DesktopRuntimeHostRef): Promise<ProjectRecord> {
      return invokeSelectedRuntimeHost(host, 'projects:archive', projectId);
    },
    restore(projectId: string, host?: DesktopRuntimeHostRef): Promise<ProjectRecord> {
      return invokeSelectedRuntimeHost(host, 'projects:restore', projectId);
    },
  },
  shellRuns: {
    async recover(sessionId: string) {
      const session = await runtimeHostSessionRef(sessionId);
      const result = await invokeWhenReady('shell-runs:recover', session.scope, session.sessionId) as
        import('../shared/runtime-host-identity.js').TerminalRecovery;
      return {
        resources: result.resources.map((update) => projectShellRunUpdate(session.scope, update)),
        closes: result.closes.map((change) => ({ ...change,
          sessionId: recordRuntimeHostSessionScope(session.scope, change.sessionId),
        })),
      };
    },
    subscribeCloseChanges(handler: (change: import('../shared/runtime-host-identity.js').TerminalCloseChange) => void) {
      return subscribeEveryRuntimeHostEvent('shell-runs:close-changed', (scope, change: import('../shared/runtime-host-identity.js').TerminalCloseChange) =>
        handler({ ...change, sessionId: recordRuntimeHostSessionScope(scope, change.sessionId) }),
      );
    },
    async list(sessionId: string): Promise<ShellRunUpdate[]> {
      const session = await runtimeHostSessionRef(sessionId);
      const updates = await invokeWhenReady(
        'shell-runs:list', session.scope, session.sessionId,
      ) as ShellRunUpdate[];
      return updates.map((update) => projectShellRunUpdate(session.scope, update));
    },
    async attach(input: {
      sessionId: string;
      ref: string;
    }): Promise<ShellRunPtySnapshot | null> {
      const session = await runtimeHostSessionRef(input.sessionId);
      const snapshot = await invokeWhenReady('shell-runs:attach', session.scope, {
        ...input,
        sessionId: session.sessionId,
      }) as ShellRunPtySnapshot | null;
      return snapshot
        ? {
            ...snapshot,
            sessionId: recordRuntimeHostSessionScope(session.scope, snapshot.sessionId),
          }
        : null;
    },
    detach(input: { sessionId: string; ref: string }): Promise<void> {
      return invokeSessionInput('shell-runs:detach', input);
    },
    async start(sessionId: string): Promise<ShellRunUpdate> {
      const session = await runtimeHostSessionRef(sessionId);
      const update = await invokeWhenReady(
        'shell-runs:start', session.scope, session.sessionId,
      ) as ShellRunUpdate;
      return projectShellRunUpdate(session.scope, update);
    },
    write(input: {
      sessionId: string;
      ref: string;
      input?: string;
      size?: { cols: number; rows: number };
    }): Promise<void> {
      return invokeSessionInput('shell-runs:write', input);
    },
    stop(input: {
      sessionId: string;
      ref: string;
    }): Promise<void> {
      return invokeSessionInput('shell-runs:stop', input);
    },
    subscribeUpdates(handler: (update: ShellRunUpdate) => void): () => void {
      return subscribeEveryRuntimeHostEvent('shell-runs:update', (scope, update: ShellRunUpdate) =>
        handler(projectShellRunUpdate(scope, update)),
      );
    },
    subscribePtyData(handler: (event: ShellRunPtyDataEvent) => void): () => void {
      return subscribeEveryRuntimeHostEvent('shell-runs:pty-data', (scope, event: ShellRunPtyDataEvent) =>
        handler({
          ...event,
          sessionId: recordRuntimeHostSessionScope(scope, event.sessionId),
        }),
      );
    },
    subscribeResync(handler: (event: { sessionId: string }) => void): () => void {
      return subscribeEveryRuntimeHostEvent('shell-runs:resync', (scope, event: { sessionId: string }) =>
        handler({
          sessionId: recordRuntimeHostSessionScope(scope, event.sessionId),
        }),
      );
    },
  },
  gitReview: {
    read(input: {
      sessionId: string;
      source: GitReviewSource;
      baseBranch?: string;
    }): Promise<GitReviewReadResult> {
      return invokeSessionInput('git-review:read', input);
    },
  },
  goal: {
    get(sessionId: string): Promise<GoalState | null> {
      return invokeProjectedSessionRuntimeHost('goal:get', sessionId);
    },
    arm(sessionId: string, goal: GoalArmRequest): Promise<GoalArmOutcome> {
      return invokeProjectedSessionRuntimeHost('goal:arm', sessionId, goal);
    },
    clear(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('goal:clear', sessionId);
    },
    pause(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('goal:pause', sessionId);
    },
    resume(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('goal:resume', sessionId);
    },
  },
  connections: {
    getSnapshot(sessionId?: string, host?: DesktopRuntimeHostRef) {
      return sessionId
        ? invokeRuntimeHostForSession('connections:getSnapshot', sessionId)
        : invokeSelectedRuntimeHost(host, 'connections:getSnapshot');
    },
    setDefault(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity | string | null, host?: DesktopRuntimeHostRef): Promise<void> {
      return invokeSelectedRuntimeHost(
        host,
        typeof connection === 'string' ? 'connections:setDefaultBySlug' : 'connections:setDefault',
        connection,
      );
    },
    setDefaultModel(input: { slug: string; model: string } | null, host?: DesktopRuntimeHostRef): Promise<void> {
      return invokeSelectedRuntimeHost(host, 'connections:setDefaultModel', input);
    },
    create(input: CreateConnectionInput, host?: DesktopRuntimeHostRef): Promise<import('@maka/core/llm-connections').IdentifiedLlmConnection> {
      return invokeSelectedRuntimeHost(host, 'connections:create', input);
    },
    verifyOnboarding(input, host) {
      return invokeSelectedRuntimeHost(host, 'connections:onboardingVerify', input);
    },
    saveOnboarding(input, host) {
      return invokeSelectedRuntimeHost(host, 'connections:onboardingSave', input);
    },
    update(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity, patch: UpdateConnectionInput, host?: DesktopRuntimeHostRef): Promise<LlmConnection> {
      return invokeSelectedRuntimeHost(host, 'connections:update', connection, patch);
    },
    delete(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity, host?: DesktopRuntimeHostRef): Promise<void> {
      return invokeSelectedRuntimeHost(host, 'connections:delete', connection);
    },
    test(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity | string, opts?: { model?: string }, host?: DesktopRuntimeHostRef): Promise<ConnectionTestResult> {
      return invokeSelectedRuntimeHost(
        host,
        typeof connection === 'string' ? 'connections:testBySlug' : 'connections:test',
        connection,
        opts,
      );
    },
    fetchModels(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity, host?: DesktopRuntimeHostRef): Promise<Pick<ModelDiscoveryResult, 'models' | 'source'>> {
      return invokeSelectedRuntimeHost(host, 'connections:fetchModels', connection);
    },
    hasSecret(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity, host?: DesktopRuntimeHostRef): Promise<boolean> {
      return invokeSelectedRuntimeHost(host, 'connections:hasSecret', connection);
    },
    getRequestHeaders(connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity, host?: DesktopRuntimeHostRef): Promise<import('@maka/core/llm-connections').SavedRequestHeaders> {
      return invokeSelectedRuntimeHost(host, 'connections:getRequestHeaders', connection);
    },
    setRequestHeaders(
      connection: import('../shared/desktop-connection-snapshot.js').DesktopConnectionIdentity,
      headers: readonly import('@maka/core/llm-connections').RequestHeaderUpdate[],
      host?: DesktopRuntimeHostRef,
    ): Promise<import('@maka/core/llm-connections').SavedRequestHeaders> {
      return invokeSelectedRuntimeHost(host, 'connections:setRequestHeaders', connection, headers);
    },
    subscribeEvents(handler: (event: ConnectionEvent) => void, host?: DesktopRuntimeHostRef): () => void {
      return host
        ? subscribeSelectedRuntimeHostEvent('connections:event', host, handler)
        : subscribeEveryRuntimeHostEvent(
            'connections:event',
            (_scope, event: ConnectionEvent) => handler(event),
          );
    },
  },
  mcp: {
    getConfig(host?: DesktopRuntimeHostRef): Promise<McpConfigFile> {
      return invokeSelectedRuntimeHost(host, 'mcp:getConfig');
    },
    listStatuses(host?: DesktopRuntimeHostRef): Promise<McpServerStatus[]> {
      return invokeSelectedRuntimeHost(host, 'mcp:listStatuses');
    },
    importConfig(source: string, host?: DesktopRuntimeHostRef): Promise<McpConfigImportResult> {
      return invokeSelectedRuntimeHost(host, 'mcp:importConfig', source);
    },
    add(serverId: string, config: McpServerConfig, host?: DesktopRuntimeHostRef): Promise<McpConfigAddResult> {
      return invokeSelectedRuntimeHost(host, 'mcp:add', serverId, config);
    },
    upsert(serverId: string, config: McpServerConfig, host?: DesktopRuntimeHostRef): Promise<McpConfigFile> {
      return invokeSelectedRuntimeHost(host, 'mcp:upsert', serverId, config);
    },
    install(serverId: string, config: McpServerConfig, host?: DesktopRuntimeHostRef): Promise<McpConfigFile> {
      return invokeSelectedRuntimeHost(host, 'mcp:install', serverId, config);
    },
    remove(serverId: string, host?: DesktopRuntimeHostRef): Promise<McpConfigFile> {
      return invokeSelectedRuntimeHost(host, 'mcp:remove', serverId);
    },
    cancelInstall(serverId: string, host?: DesktopRuntimeHostRef): Promise<McpConfigFile> {
      return invokeSelectedRuntimeHost(host, 'mcp:cancelInstall', serverId);
    },
    test(serverId: string, host?: DesktopRuntimeHostRef): Promise<McpTestResult> {
      return invokeSelectedRuntimeHost(host, 'mcp:test', serverId);
    },
    // Same scoped seam as every other MCP method: the handlers live on the
    // Runtime Host's ScopedIpcMain, whose first argument is the host ref —
    // a raw invoke would put serverId in that slot and fail the scope check
    // before the handler ever ran.
    login(serverId: string, host?: DesktopRuntimeHostRef): Promise<McpServerStatus> {
      return invokeSelectedRuntimeHost(host, 'mcp:login', serverId);
    },
    cancelLogin(serverId: string, host?: DesktopRuntimeHostRef): Promise<boolean> {
      return invokeSelectedRuntimeHost(host, 'mcp:cancelLogin', serverId);
    },
    logout(serverId: string, host?: DesktopRuntimeHostRef): Promise<McpServerStatus> {
      return invokeSelectedRuntimeHost(host, 'mcp:logout', serverId);
    },
    subscribeChanges(handler: (statuses: McpServerStatus[]) => void): () => void {
      return subscribeActiveRuntimeHostEvent('mcp:changed', handler);
    },
  },
  // PR110b: onboarding snapshot + milestone IPCs. Renderer polls
  // `getSnapshot()` on app load and re-polls on existing invalidations.
  // Onboarding state and connection setup belong to the default Host; bounded
  // Owner send outcomes are merged from ready Owner Hosts; Guest summaries
  // come from the separate, authorized mount catalog.
  onboarding: {
    getSnapshot(): Promise<OnboardingSnapshot> {
      return loadDesktopOnboardingSnapshot();
    },
    async setMilestone(
      id: OnboardingMilestoneId,
      status: 'completed' | 'skipped',
      host?: DesktopRuntimeHostRef,
    ): Promise<OnboardingSnapshot> {
      const scope = await selectedRuntimeHostScope(host);
      const snapshot = await invokeWhenReady(
        'onboarding:setMilestone', scope, id, status,
      ) as OnboardingSnapshot;
      return projectOnboardingSnapshot(scope, snapshot);
    },
  },
  taskReadiness: {
    getSnapshot(input?: DesktopTaskSubmissionReadinessRequest, sessionId?: string) {
      return sessionId
        ? invokeRuntimeHostForSession('taskReadiness:getSnapshot', sessionId, input)
        : invokeActiveRuntimeHost('taskReadiness:getSnapshot', input);
    },
  },
  permissions: {
    ensureSandbox(target: { sessionId: string } | { host: DesktopRuntimeHostRef }) {
      return 'sessionId' in target
        ? invokeRuntimeHostForSession<boolean>('permissions:ensureSandbox', target.sessionId)
        : invokeSelectedRuntimeHost<boolean>(target.host, 'permissions:ensureSandbox');
    },
    getSnapshot(host?: DesktopRuntimeHostRef): Promise<PermissionSnapshot> {
      return invokeSelectedRuntimeHost(host, 'permissions:getSnapshot');
    },
    openSystemSettings(permId: string, host?: DesktopRuntimeHostRef): Promise<PermissionActionResult> {
      return invokeSelectedRuntimeHost(host, 'permissions:openSystemSettings', permId);
    },
    requestAccess(permId: string, host?: DesktopRuntimeHostRef): Promise<PermissionActionResult> {
      return invokeSelectedRuntimeHost(host, 'permissions:requestAccess', permId);
    },
    startDragOnboarding(permId: string, host?: DesktopRuntimeHostRef): Promise<PermissionOverlayStartResult> {
      return invokeSelectedRuntimeHost(host, 'permissions:startDragOnboarding', permId);
    },
  },
  capabilities: {
    getSnapshot(host?: DesktopRuntimeHostRef): Promise<CapabilitySnapshotCollection> {
      return invokeSelectedRuntimeHost(host, 'capabilities:getSnapshot');
    },
  },
  health: {
    getSnapshot(host?: DesktopRuntimeHostRef): Promise<HealthSnapshot> {
      return invokeSelectedRuntimeHost(host, 'health:getSnapshot');
    },
  },
  memory: {
    getState(sessionId?: string, host?: DesktopRuntimeHostRef): Promise<LocalMemoryState> {
      return sessionId
        ? invokeRuntimeHostForSession('memory:getState', sessionId)
        : invokeSelectedRuntimeHost(host, 'memory:getState');
    },
    save(content: string, host?: DesktopRuntimeHostRef): Promise<LocalMemoryState> {
      return invokeSelectedRuntimeHost(host, 'memory:save', content);
    },
    reset(host?: DesktopRuntimeHostRef): Promise<LocalMemoryState> {
      return invokeSelectedRuntimeHost(host, 'memory:reset');
    },
    restoreLatestBackup(host?: DesktopRuntimeHostRef): Promise<{ ok: true; state: LocalMemoryState } | { ok: false; state: LocalMemoryState; code: string }> {
      return invokeSelectedRuntimeHost(host, 'memory:restoreLatestBackup');
    },
    restoreBackup(kind: 'save' | 'reset' | 'restore', host?: DesktopRuntimeHostRef): Promise<{ ok: true; state: LocalMemoryState } | { ok: false; state: LocalMemoryState; code: string }> {
      return invokeSelectedRuntimeHost(host, 'memory:restoreBackup', kind);
    },
    setEnabled(enabled: boolean, host?: DesktopRuntimeHostRef): Promise<LocalMemoryState> {
      return invokeSelectedRuntimeHost(host, 'memory:setEnabled', enabled);
    },
    setAgentReadEnabled(enabled: boolean, host?: DesktopRuntimeHostRef): Promise<LocalMemoryState> {
      return invokeSelectedRuntimeHost(host, 'memory:setAgentReadEnabled', enabled);
    },
    openFile(host?: DesktopRuntimeHostRef): Promise<{ ok: true } | { ok: false; code: string }> {
      return invokeSelectedRuntimeHost(host, 'memory:openFile');
    },
    openLatestBackup(host?: DesktopRuntimeHostRef): Promise<{ ok: true } | { ok: false; code: string }> {
      return invokeSelectedRuntimeHost(host, 'memory:openLatestBackup');
    },
    openBackup(kind: 'save' | 'reset' | 'restore', host?: DesktopRuntimeHostRef): Promise<{ ok: true } | { ok: false; code: string }> {
      return invokeSelectedRuntimeHost(host, 'memory:openBackup', kind);
    },
  },
  attachments: {
    async prepare(sessionId: string, items: RendererIngestInput[]) {
      const session = await runtimeHostSessionRef(sessionId);
      let encoded: Awaited<ReturnType<typeof encodeIngestItems>>;
      try { encoded = await encodeIngestItems(items); }
      catch (error) {
        if (error instanceof AttachmentIngestBlockedError) return { ok: false, code: error.code };
        throw error;
      }
      const result = await invokeWhenReady('attachments:prepare', session.scope, session.sessionId, encoded) as PrepareAttachmentsResult;
      return result.ok ? { ok: true, attachments: projectDesktopAttachmentRefs(session.scope, result.attachments) } : result;
    },
    pickDirectory: () => invokeWhenReady('directories:pick'),
    pickFiles(): Promise<
      | {
          ok: true;
          files: {
            approvalId: string;
            name: string;
            mimeType?: string;
            size: number;
          }[];
        }
      | { ok: false; reason: 'cancelled' }
    > {
      return invokeWhenReady('attachments:pickFiles');
    },
    // Staged-attachment thumbnail for the composer drawer. Peeks the approval
    // (never consumes it) so the token stays redeemable for the actual send.
    previewApproval(approvalId: string): Promise<
      | { ok: true; base64: string; mimeType: string }
      | { ok: false; reason: string }
    > {
      return invokeWhenReady('attachments:previewApproval', approvalId);
    },
    readBytes(sessionId: string, artifactId: string): Promise<ArtifactBinaryReadResult> {
      return invokeSessionRuntimeHost('attachments:readBytes', sessionId, artifactId);
    },
  },
  search: createThreadSearchClient({
    // Search each ready Owner Host independently; Guests cannot search a workspace.
    scopes: readyOwnerRuntimeHostScopes,
    async search(scope, request, requestId) {
      const result = await invokeWhenReady('search:thread', scope, request, requestId) as
        | SearchResult[]
        | { ok: false; reason: SearchErrorReason; message: string };
      return Array.isArray(result)
        ? result.map((entry) =>
            entry.target?.kind === 'thread'
              ? {
                  ...entry,
                  target: {
                    ...entry.target,
                    sessionId: recordRuntimeHostSessionScope(scope, entry.target.sessionId),
                  },
                }
              : entry,
          )
        : result;
    },
    cancel: (scope, requestId) => invokeWhenReady('search:thread:cancel', scope, requestId),
  }),
  // Browser-assisted Codex account bridge. NEVER returns raw OAuth
  // credentials; the renderer only sees account state and action results.
  //
  // kenji `1da909d5`/`45b31e16` hardening: `openAuthUrl` takes ONLY an
  // `authRequestId`; the URL is held by main from the earlier `getAuthUrl`
  // call. Renderer can never hand `shell.openExternal` an arbitrary URL.
  openAiCodex: {
    getAuthUrl(host: DesktopRuntimeHostRef | undefined, target: DesktopOAuthLoginTarget) {
      return invokeSelectedRuntimeHost(host, 'openai-codex:get-auth-url', target);
    },
    openAuthUrl(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:open-auth-url', authRequestId);
    },
    completeAuthorization(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<DesktopOAuthAuthorizationResult> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:complete-authorization', authRequestId);
    },
    cancelAuthorization(authRequestId?: string, host?: DesktopRuntimeHostRef): Promise<{ ok: true }> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:cancel-authorization', authRequestId);
    },
    getAccountState(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<{
      provider: 'openai-codex';
      runtimeState: 'not_logged_in' | 'authorizing' | 'authenticated' | 'refreshing' | 'refresh_failed';
      accountId?: string;
      email?: string;
      plan?: string;
      picture?: string;
      errorMessage?: string;
    }> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:get-account-state', connectionId);
    },
    getEnrollmentState(host?: DesktopRuntimeHostRef): Promise<{ enabled: boolean }> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:get-enrollment-state');
    },
    refreshTokens(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:refresh-tokens', connectionId);
    },
    logout(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'openai-codex:logout', connectionId);
    },
  },
  xaiOAuth: {
    getAuthUrl(host: DesktopRuntimeHostRef | undefined, target: DesktopOAuthLoginTarget) {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:get-auth-url', target);
    },
    openAuthUrl(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:open-auth-url', authRequestId);
    },
    completeAuthorization(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<DesktopOAuthAuthorizationResult> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:complete-authorization', authRequestId);
    },
    cancelAuthorization(authRequestId?: string, host?: DesktopRuntimeHostRef): Promise<{ ok: true }> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:cancel-authorization', authRequestId);
    },
    getAccountState(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<{
      provider: 'xai-oauth';
      runtimeState:
        | 'not_logged_in'
        | 'authorizing'
        | 'authenticated'
        | 'refreshing'
        | 'refresh_failed'
        | 'storage_failed';
      errorMessage?: string;
    }> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:get-account-state', connectionId);
    },
    getEnrollmentState(host?: DesktopRuntimeHostRef): Promise<{ enabled: boolean }> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:get-enrollment-state');
    },
    refreshTokens(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:refresh-tokens', connectionId);
    },
    logout(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'xai-oauth:logout', connectionId);
    },
  },
  githubCopilotSubscription: {
    connectExistingLogin(host?: DesktopRuntimeHostRef): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:connect-existing-login');
    },
    getAuthUrl(host: DesktopRuntimeHostRef | undefined, target: DesktopOAuthLoginTarget) {
      return invokeSelectedRuntimeHost(host, 'github-copilot:get-auth-url', target);
    },
    openAuthUrl(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:open-auth-url', authRequestId);
    },
    completeAuthorization(authRequestId: string, host?: DesktopRuntimeHostRef): Promise<DesktopOAuthAuthorizationResult> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:complete-authorization', authRequestId);
    },
    cancelAuthorization(authRequestId?: string, host?: DesktopRuntimeHostRef): Promise<{ ok: true }> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:cancel-authorization', authRequestId);
    },
    getAccountState(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<{
      provider: 'github-copilot';
      runtimeState: 'not_logged_in' | 'authorizing' | 'authenticated' | 'refreshing' | 'refresh_failed' | 'storage_failed';
      errorMessage?: string;
    }> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:get-account-state', connectionId);
    },
    getEnrollmentState(host?: DesktopRuntimeHostRef): Promise<{ enabled: boolean }> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:get-enrollment-state');
    },
    refreshTokens(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:refresh-tokens', connectionId);
    },
    logout(host: DesktopRuntimeHostRef | undefined, connectionId: string): Promise<SubscriptionActionResult> {
      return invokeSelectedRuntimeHost(host, 'github-copilot:logout', connectionId);
    },
  },
  externalAgents: {
    selectExecutable(host) { return invokeSelectedRuntimeHost(host, 'external-agents:select-executable'); },
    start(input, host) { return invokeSelectedRuntimeHost(host, 'external-agents:setup:start', input); },
    query(attemptId, host) { return invokeSelectedRuntimeHost(host, 'external-agents:setup:query', { attemptId }); },
    cancel(attemptId, host) { return invokeSelectedRuntimeHost(host, 'external-agents:setup:cancel', { attemptId }); },
  },
  settings: {
    getClient(): Promise<AppSettings> {
      return invokeWhenReady('settings:client:get');
    },
    get(host?: DesktopRuntimeHostRef): Promise<RuntimeHostAppSettings> {
      return invokeSelectedRuntimeHost(host, 'settings:get');
    },
    updateClient(patch: UpdateAppSettingsInput): Promise<UpdateAppSettingsResult> {
      return invokeWhenReady('settings:client:update', patch);
    },
    update(
      patch: UpdateAppSettingsInput,
      host?: DesktopRuntimeHostRef,
      guard?: RuntimeHostSettingsUpdateGuard,
    ): Promise<UpdateAppSettingsResult<RuntimeHostAppSettings>> {
      return invokeSelectedRuntimeHost(host, 'settings:update', patch, guard);
    },
    subscribeClientChanged(handler: () => void): () => void {
      const listener = () => handler();
      ipcRenderer.on('settings:clientChanged', listener);
      return () => ipcRenderer.off('settings:clientChanged', listener);
    },
    subscribeExternalChanged(handler: () => void, host?: DesktopRuntimeHostRef): () => void {
      return subscribeSelectedRuntimeHostEvent('settings:externalChanged', host, handler);
    },
    testNetworkProxy(input?: TestProxyInput, host?: DesktopRuntimeHostRef): Promise<SettingsTestResult> {
      return invokeSelectedRuntimeHost(host, 'settings:testNetworkProxy', input);
    },
    testBotChannel(provider: BotProvider): Promise<SettingsTestResult> {
      return invokeWhenReady('settings:testBotChannel', provider);
    },
    async usageStats(
      range?: UsageRange | Extract<UsageScreenRequest, {kind: 'activity'}>,
      host?: DesktopRuntimeHostRef,
      query?: UsageScreenQuery,
    ): Promise<UsageStats | UsageScreenResult> {
      const scope = await selectedRuntimeHostScope(host);
      if (range && typeof range === 'object') {
        const result = await invokeWhenReady('usage:activity', scope, range) as UsageScreenResult;
        return result.kind === 'activity'
          ? {...result, page: {...result.page, logs: projectDesktopUsageActivity(scope, result.page.logs)}}
          : result;
      }
      const stats = await invokeWhenReady('settings:usageStats', scope, range, query) as UsageStats | Extract<UsageScreenFailure, {kind: 'screen_response_too_large'}>;
      if ('kind' in stats) return stats;
      return projectDesktopUsageStats(scope, stats);
    },
    bots: {
      listStatuses(): Promise<Record<BotProvider, BotStatus>> {
        return invokeWhenReady('settings:bots:listStatuses');
      },
      restart(provider: BotProvider): Promise<BotStatus> {
        return invokeWhenReady('settings:bots:restart', provider);
      },
      wechatQrCode(): Promise<WechatBridgeQrCodeResult> {
        return invokeWhenReady('settings:bots:wechatQrCode');
      },
      subscribeStatusChanges(handler: (status: BotStatus) => void): () => void {
        const listener = (_event: Electron.IpcRendererEvent, status: BotStatus) => handler(status);
        ipcRenderer.on('settings:bots:statusChanged', listener);
        return () => ipcRenderer.off('settings:bots:statusChanged', listener);
      },
      onboarding: {
        start(input: BotOnboardingStartInput): Promise<Result<BotOnboardingSnapshot>> {
          return invokeWhenReady('settings:bots:onboarding:start', input);
        },
        poll(sessionId: string): Promise<Result<BotOnboardingSnapshot>> {
          return invokeWhenReady('settings:bots:onboarding:poll', sessionId);
        },
        cancel(sessionId: string): Promise<Result<BotOnboardingSnapshot>> {
          return invokeWhenReady('settings:bots:onboarding:cancel', sessionId);
        },
        openInBrowser(sessionId: string): Promise<Result<void>> {
          return invokeWhenReady('settings:bots:onboarding:open', sessionId);
        },
      },
    },
  },
  notifications: {
    // Fire-and-forget signal that an agent turn reached a terminal
    // state. `title` is the session name, `body` the start of the reply
    // (or the error message); main sanitizes both and falls back to
    // generic copy when blank. Main gates on the product toggle + window
    // focus before raising a native OS notification.
    runEnded(payload: {
      kind: 'completed' | 'errored';
      title?: string;
      body?: string;
    }): Promise<void> {
      return invokeWhenReady('notifications:runEnded', payload);
    },
  },
  inspector: {
    /** Read-only per-session causal trace (#1625). Never writes runtime state. */
    trace(sessionId: string, cursor?: string): Promise<Result<DesktopSessionTracePage>> {
      return bridgeResult(() => loadSessionTracePage(sessionId, cursor), 'INSPECTOR_TRACE_FAILED');
    },
    summary(sessionId: string): Promise<Result<DesktopSessionUsageSummary>> {
      return loadSessionUsageSummary(sessionId);
    },
    subscribeUsageChanges(sessionId: string, handler: () => void): () => void {
      let disposed = false;
      let unsubscribe = () => {};
      void runtimeHostSessionRef(sessionId)
        .then(({ scope, sessionId: rawSessionId }) => {
          if (disposed) return;
          unsubscribe = subscribeRuntimeHostEvent(
            'usage:changed',
            scope,
            (event: { sessionId: string }) => {
              if (event.sessionId === rawSessionId) handler();
            },
          );
        })
        .catch(() => undefined);
      return () => {
        disposed = true;
        unsubscribe();
      };
    },
    /**
     * What the session's context is made of right now (#2323).
     *
     * A different question from "what happened in this session", and it has
     * its own typed owner on the Host — the same snapshot `/context` prints.
     * The Inspector asks that owner rather than widening the trace, so the two
     * surfaces cannot drift into two implementations of one fact.
     */
    context(sessionId: string): Promise<Result<ContextDiagnosticsResult>> {
      return bridgeResult(
        async () => {
          const session = await runtimeHostSessionRef(sessionId);
          return scopedRuntimeHost(session.scope).query('context.diagnostics.query', {
            sessionId: session.sessionId,
          });
        },
        'INSPECTOR_CONTEXT_FAILED',
      );
    },
  },
  dailyReview: {
    day(offsetDays: number, daySpan?: number, host?: DesktopRuntimeHostRef): Promise<Result<DailyReviewSummary>> {
      return bridgeResult(async () => {
        const scope = await selectedRuntimeHostScope(host);
        const result = await scopedRuntimeHost(scope).query('daily-review.query', {
          kind: 'summary',
          offsetDays: integer(offsetDays, 0),
          daySpan: Math.max(1, Math.min(30, integer(daySpan, 1))),
        });
        if (result.kind !== 'summary') throw new Error('Invalid Daily Review summary');
        return projectDesktopDailyReviewSummary(scope, result.summary);
      }, 'DAILY_REVIEW_DAY_FAILED');
    },
    async getConfig(host?: DesktopRuntimeHostRef): Promise<DailyReviewConfig> {
      const result = await scopedRuntimeHost(
        await selectedRuntimeHostScope(host),
      ).query('daily-review.query', {
        kind: 'config',
      });
      if (result.kind !== 'config') throw new Error('Invalid Daily Review config');
      return result.config;
    },
    setConfig(patch: Partial<DailyReviewConfig>, host?: DesktopRuntimeHostRef): Promise<DailyReviewConfig> {
      return updateDailyReviewConfig(patch, host);
    },
    async runOnce(input: { range: DailyReviewRange; offsetDays?: number; modelKey?: string }): Promise<{ archiveId: string }> {
      const result = await runtimeHost.command('daily-review.mutate', {
        kind: 'run',
        range: DAILY_REVIEW_RANGES.includes(input.range) ? input.range : 1,
        offsetDays: integer(input.offsetDays, 0),
        modelKeyOverride: input.modelKey ?? '',
        replaceExisting: false,
      });
      if (result.kind !== 'archive') throw new Error('Invalid Daily Review run');
      return { archiveId: result.archive.id };
    },
    listArchives(): Promise<DailyReviewArchiveSummary[]> {
      return listDailyReviewArchives();
    },
    async getArchive(archiveId: string): Promise<DailyReviewArchive | null> {
      const result = await runtimeHost.query('daily-review.query', {
        kind: 'archive',
        archiveId,
      });
      if (result.kind !== 'archive') throw new Error('Invalid Daily Review archive');
      return result.archive;
    },
    /**
     * PR-DAILY-REVIEW-EXPORT-FILE-0: render the markdown in the renderer
     * (where the human-readable title context lives) and ship the bytes
     * to main for the save dialog + write. Main never sees the raw
     * telemetry; only the formatted output.
     */
    saveMarkdownToFile(input: {
      markdown: string;
      defaultName: string;
    }): Promise<
      { ok: true; path: string } | { ok: false; reason: 'canceled' | 'write_failed' | 'invalid_input' }
    > {
      return invokeWhenReady('daily-review:saveMarkdownToFile', input);
    },
  },
  appWindow: {
    quit(): Promise<void> { return invokeWhenReady('window:quit'); },
    popupMenu(input: import('../shared/native-menu.js').NativeMenuRequest): Promise<string | null> {
      return invokeWhenReady('window:popupMenu', input);
    },
    setTitlebarControlsVisible(visible: boolean): Promise<void> {
      return invokeWhenReady('window:setTitlebarControlsVisible', visible);
    },
    setThemeSource(themePref: ThemePreference): Promise<void> {
      return invokeWhenReady('window:setThemeSource', themePref);
    },
    // PR-WINDOW-TITLEBAR-0: re-sync the native Windows titleBarOverlay
    // color/symbolColor to the resolved app surface. No-op on non-Windows.
    setTitleBarOverlayTheme(theme: { isDark: boolean; backgroundColor: string }): Promise<void> {
      return invokeWhenReady('window:setTitleBarOverlayTheme', theme);
    },
    // PR-SHOW-AFTER-FIRST-COMMIT: tell main the renderer finished its first
    // React commit so the hidden window can be revealed. Fire-and-forget.
    notifyRendererReady(): Promise<void> {
      return invokeWhenReady('window:notifyRendererReady');
    },
    // PR-2088: main-to-renderer route for native-menu commands (New Task /
    // Settings / Keyboard Shortcuts). The `ipcRenderer.on`/`off` idiom keeps
    // an HMR or shell remount from stacking duplicate listeners.
    subscribeCommand(handler: (command: WindowCommand) => void): () => void {
      const listener = (_event: Electron.IpcRendererEvent, command: WindowCommand) => handler(command);
      ipcRenderer.on('window:command', listener);
      return () => ipcRenderer.off('window:command', listener);
    },
  },
  config: {
    export(input: { categories: ConfigCategory[] }, host?: DesktopRuntimeHostRef): Promise<
      | { ok: false; reason: 'no_categories' | 'canceled' }
      | { ok: true; path: string; includedData: ConfigCategory[] }
    > {
      return invokeSelectedRuntimeHost(host, 'config:export', input);
    },
    import(input: { strategy: 'skip' | 'overwrite' }, host?: DesktopRuntimeHostRef): Promise<
      | { ok: false; reason: 'canceled' | 'not_json' | 'malformed' | 'unsupported_version'; message?: string }
      | {
          ok: true;
          includedData: ConfigCategory[];
          result: {
            connections?: {
              created: number;
              overwritten: number;
              skipped: number;
            };
            settings?: { applied: boolean };
            credentials?: { applied: number; skipped: number };
            memory?: { applied: boolean };
          };
        }
    > {
      return invokeSelectedRuntimeHost(host, 'config:import', input);
    },
  },
  app: {
    info(host?: DesktopRuntimeHostRef): Promise<DesktopAppInfo> {
      return invokeSelectedRuntimeHost(host, 'app:info');
    },
    iconPreviews(): Promise<ReadonlyArray<{ id: AppIconChoice; dataUrl: string; removable?: boolean }>> {
      return invokeWhenReady('app:iconPreviews');
    },
    selectIcon(icon: AppIconChoice, target?: AppIconTarget): Promise<AppIconSelectResult> {
      return invokeWhenReady('app:selectIcon', icon, target);
    },
    importIcon(): Promise<AppIconImportResult> {
      return invokeWhenReady('app:importIcon');
    },
    removeIcon(icon: AppIconChoice): Promise<AppIconRemoveResult> {
      return invokeWhenReady('app:removeIcon', icon);
    },
    subscribeUpdateStatus(handler: (status: AppUpdateStatus) => void): () => void {
      const listener = (_event: Electron.IpcRendererEvent, status: AppUpdateStatus) => handler(status);
      ipcRenderer.on('app:updateStatusChanged', listener);
      return () => ipcRenderer.off('app:updateStatusChanged', listener);
    },
    updateStatus(): Promise<AppUpdateStatus> {
      return invokeWhenReady('app:updateStatus');
    },
    checkForUpdates(): Promise<AppUpdateStatus> {
      return invokeWhenReady('app:checkForUpdates');
    },
    retryUpdateDownload(): Promise<AppUpdateStatus> {
      return invokeWhenReady('app:retryUpdateDownload');
    },
    installUpdate(input: AppUpdateInstallRequest): Promise<AppUpdateInstallResult> {
      return invokeWhenReady('app:installUpdate', input);
    },
    sessionProjectInfo(sessionId: string): Promise<{
      projectPath: string;
      projectGit: { isGitRepo: boolean; branch?: string };
    }> {
      return invokeSessionRuntimeHost('app:sessionProjectInfo', sessionId);
    },
    openPath(
      key: 'workspace' | 'memory' | 'project',
      sessionId?: string,
      host?: DesktopRuntimeHostRef,
    ): Promise<
      | { ok: true; opened: string }
      | {
          ok: false;
          reason: 'unknown-key' | 'not-allowed' | 'missing' | 'not-a-directory' | 'open-failed';
        }
    > {
      if (!sessionId) return invokeSelectedRuntimeHost(host, 'app:openPath', key, undefined);
      return runtimeHostSessionRef(sessionId).then((session) =>
        invokeWhenReady('app:openPath', session.scope, key, session.sessionId),
      );
    },
    resolveProjectGitInfo(projectPath: string, host?: DesktopRuntimeHostRef): Promise<
      | { ok: true; projectPath: string; projectGit: { isGitRepo: boolean; branch?: string } }
      | { ok: false; reason: 'invalid-path' | 'not-found' }
    > {
      return invokeSelectedRuntimeHost(host, 'app:resolveProjectGitInfo', projectPath);
    },
    openArtifactPath(
      sessionId: string,
      artifactId: string,
    ): Promise<
      | { ok: true; opened: string }
      | {
          ok: false;
          reason: 'unknown-key' | 'not-allowed' | 'missing' | 'not-a-directory' | 'open-failed';
        }
    > {
      return invokeSessionRuntimeHost('app:openArtifactPath', sessionId, artifactId);
    },
    showArtifactInFolder(
      sessionId: string,
      artifactId: string,
    ): Promise<
      | { ok: true; opened: string }
      | {
          ok: false;
          reason: 'unknown-key' | 'not-allowed' | 'missing' | 'not-a-directory' | 'open-failed';
        }
    > {
      return invokeSessionRuntimeHost('app:showArtifactInFolder', sessionId, artifactId);
    },
    saveArtifactAs(sessionId: string, artifactId: string): Promise<ArtifactSaveResult> {
      return invokeSessionRuntimeHost('app:saveArtifactAs', sessionId, artifactId);
    },
  },
  diagnostics: {
    takePreviousMainProcessInterruption(): Promise<boolean> {
      previousMainProcessInterruptionRead ??= invokeWhenReady(
        'diagnostics:takePreviousMainProcessInterruption',
      ) as Promise<boolean>;
      return previousMainProcessInterruptionRead;
    },
    copyPreviousMainProcessInterruption(): Promise<void> {
      return invokeWhenReady('diagnostics:copyPreviousMainProcessInterruption');
    },
    async copyReport(input: DesktopDiagnosticInput): Promise<void> {
      const rendererContext = {
        rendererUserAgent: navigator.userAgent,
        rendererLocale: navigator.language,
      };
      if (input.surface === 'manual') {
        const { target, ...manualInput } = input;
        const resolution = await resolveManualDiagnosticRuntimeHost(target);
        const wireInput: DesktopManualDiagnosticWireInput = {
          ...manualInput,
          hostTarget: resolution.hostTarget,
          ...rendererContext,
        };
        await invokeWhenReady(
          'diagnostics:copyReport',
          resolution.scope,
          wireInput,
        );
        return;
      }
      if (input.surface === 'renderer_crash') {
        const wireInput: DesktopErrorDiagnosticWireInput = {
          surface: 'renderer_crash',
          title: input.title,
          ...(input.description ? { description: input.description } : {}),
          ...(input.details ? { details: input.details } : {}),
          hostTarget: 'none',
          ...rendererContext,
        };
        await invokeWhenReady('diagnostics:copyReport', undefined, wireInput);
        return;
      }
      const { target, ...errorInput } = input;
      const parsedTarget = target ? parseDiagnosticTarget(target) : undefined;
      if (!parsedTarget) {
        const wireInput: DesktopErrorDiagnosticWireInput = {
          ...errorInput,
          hostTarget: 'none',
          ...rendererContext,
        };
        await invokeWhenReady('diagnostics:copyReport', undefined, wireInput);
        return;
      }
      const resolution = await resolveTaskDiagnosticRuntimeHost(parsedTarget.selector);
      const wireInput: DesktopErrorDiagnosticWireInput = {
        ...errorInput,
        hostTarget: 'task',
        ...rendererContext,
        ...(parsedTarget.execution ? { execution: parsedTarget.execution } : {}),
      };
      await invokeWhenReady('diagnostics:copyReport', resolution.scope, wireInput);
    },
  },
  workspace: {
    /** Composer `@` mention popup: list workspace files matching `query`. */
    searchFiles(
      query: string,
      options?: { sessionId?: string; limit?: number },
    ): Promise<
      | { ok: true; files: Array<{ relativePath: string }> }
      | { ok: false; reason: 'no_project' | 'search_failed' }
    > {
      return options?.sessionId
        ? invokeSessionInput('workspace:searchFiles', { query, ...options } as {
            query: string;
            sessionId: string;
            limit?: number;
          })
        : invokeActiveRuntimeHost('workspace:searchFiles', { query, ...options });
    },
  },
  e2eFixture: {
    async getState(): Promise<E2eFixtureState | null> {
      const state = await invokeWhenReady('e2eFixture:getState') as E2eFixtureState | null;
      if (!state?.activeSessionId) return state;
      const scope = await activeRuntimeHostRef();
      return {
        ...state,
        activeSessionId: recordRuntimeHostSessionScope(scope, state.activeSessionId),
      };
    },
  },
  artifacts: {
    list(sessionId: string): Promise<ArtifactDescriptor[]> {
      return invokeProjectedSessionRuntimeHost('artifacts:list', sessionId);
    },
    readText(sessionId: string, artifactId: string): Promise<ArtifactTextReadResult> {
      return invokeSessionRuntimeHost('artifacts:readText', sessionId, artifactId);
    },
    readBinary(sessionId: string, artifactId: string): Promise<ArtifactBinaryReadResult> {
      return invokeSessionRuntimeHost('artifacts:readBinary', sessionId, artifactId);
    },
    delete(sessionId: string, artifactId: string): Promise<void> {
      return invokeSessionRuntimeHost('artifacts:delete', sessionId, artifactId);
    },
  },
  // Embedded browser (P3). The native WebContentsView floats above the DOM; the
  // renderer panel only mirrors its strip's rect and drives navigation. No
  // automation endpoint/secret is ever exposed here — that stays main-internal.
  browser: {
    /** Tell main which conversation this window shows, so it can validate targets. */
    setActiveSession(sessionId: string | null): void {
      browserSelection.setActiveSession(sessionId);
    },
    /** Mirror the panel strip's on-screen rect (null hides the native view). */
    setViewport(input: { sessionId: string; rect: BrowserViewRect | null }): void {
      browserSelection.setViewport(input);
    },
    capturePage(sessionId: string): Promise<string | undefined> {
      return browserSelection.capturePage(sessionId);
    },
    navigate(sessionId: string, url: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:navigate', sessionId, url);
    },
    back(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:back', sessionId);
    },
    forward(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:forward', sessionId);
    },
    reload(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:reload', sessionId);
    },
    stop(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:stop', sessionId);
    },
    close(sessionId: string): Promise<void> {
      return invokeSessionRuntimeHost('browser:close-page', sessionId);
    },
    getState(sessionId: string): Promise<BrowserState | null> {
      return invokeSessionRuntimeHost('browser:get-state', sessionId);
    },
    onState(handler: (payload: { sessionId: string; state: BrowserState }) => void): () => void {
      return subscribeEveryRuntimeHostEvent(
        'browser:state',
        (scope, payload: { sessionId: string; state: BrowserState }) =>
        handler({
          ...payload,
          sessionId: recordRuntimeHostSessionScope(scope, payload.sessionId),
        }),
      );
    },
    onLive(handler: (payload: { sessionIds: string[] }) => void): () => void {
      return subscribeEveryRuntimeHostEvent(
        'browser:live',
        (scope, payload: { sessionIds: string[] }) =>
        handler({
          sessionIds: payload.sessionIds.map((sessionId) =>
            recordRuntimeHostSessionScope(scope, sessionId),
          ),
        }),
      );
    },
  },
} satisfies MakaBridge;

// E2E-only async controls. Real users never get these: the preload mirrors the
// main process's isolated-E2E gate (startup-context.ts) — MAKA_E2E alone is
// not enough without the throwaway profile dir. An armed latch holds the next
// bridge call until the test releases
// it, while a settled-call waiter exposes a deterministic completion boundary
// for work whose visible result may intentionally keep the same DOM identity.
// The wrappers must be installed BEFORE
// exposeInMainWorld: the bridge is cloned into the main world at expose time,
// and the exposed clone is sealed against later patching.
if (process.env.MAKA_E2E === '1' && process.env.MAKA_E2E_USER_DATA_DIR) {
  type LatchKey = 'sessions.list' | 'sessions.observe';
  const gates = new Map<LatchKey, { promise: Promise<void>; oneShot: boolean }>();
  const releases = new Map<LatchKey, { resolve: () => void; reject: (error: Error) => void }>();
  let nextSessionObservationError: Error | undefined;
  let nextTranscriptOpenError: Error | undefined;
  const waitForLatch = async (key: LatchKey): Promise<void> => {
    const gate = gates.get(key);
    if (!gate) return;
    if (gate.oneShot) gates.delete(key);
    await gate.promise;
  };
  const wrapLatched = <Args extends unknown[], Result>(
    call: (...args: Args) => Promise<Result>,
    key: LatchKey,
  ) => async (...args: Args): Promise<Result> => {
    await waitForLatch(key);
    return call(...args);
  };
  makaBridge.sessions.list = wrapLatched(
    makaBridge.sessions.list.bind(makaBridge.sessions),
    'sessions.list',
  );
  const subscribeSessionEvents = makaBridge.sessions.subscribeEvents.bind(makaBridge.sessions);
  makaBridge.sessions.subscribeEvents = (
    sessionId,
    handler,
    onObservationSeed,
    onSeedError,
    onExecution,
  ) => {
    const nextError = nextSessionObservationError;
    nextSessionObservationError = undefined;
    let disposed = false;
    if (!nextError) {
      let unsubscribe = () => {};
      void waitForLatch('sessions.observe').then(() => {
        if (!disposed) unsubscribe = subscribeSessionEvents(
          sessionId, handler, onObservationSeed, onSeedError, onExecution,
        );
      });
      return () => { disposed = true; unsubscribe(); };
    }
    void Promise.resolve().then(() => {
      if (!disposed) onSeedError?.(nextError);
    });
    return () => {
      disposed = true;
    };
  };
  const openTranscript = makaBridge.transcripts.open.bind(makaBridge.transcripts);
  makaBridge.transcripts.open = (...args) => {
    const nextError = nextTranscriptOpenError;
    nextTranscriptOpenError = undefined;
    return nextError ? Promise.reject(nextError) : openTranscript(...args);
  };
  contextBridge.exposeInMainWorld('makaE2eLatch', {
    arm(key: LatchKey, options?: { oneShot?: boolean }) {
      let resolve: () => void = () => {};
      let reject: (error: Error) => void = () => {};
      const promise = new Promise<void>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
      });
      gates.set(key, { promise, oneShot: options?.oneShot === true });
      releases.set(key, { resolve, reject });
    },
    rejectNextSessionObservation(message: string) {
      nextSessionObservationError = new Error(message);
    },
    rejectNextTranscriptOpen(message: string) {
      nextTranscriptOpenError = new Error(message);
    },
    release(key: LatchKey) {
      releases.get(key)?.resolve();
      releases.delete(key);
      gates.delete(key);
    },
    reject(key: LatchKey, message: string) {
      releases.get(key)?.reject(new Error(message));
      releases.delete(key);
      gates.delete(key);
    },
  });
}

contextBridge.exposeInMainWorld('maka', makaBridge);
