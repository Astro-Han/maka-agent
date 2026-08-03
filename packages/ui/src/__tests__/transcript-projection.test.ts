import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type {
  ShellRunSnapshotResult,
  ShellRunToolResult,
  ShellRunUpdate,
  StoredMessage,
} from '@maka/core';
import { createTranscriptProjection } from '../transcript-projection.js';
import { timelineTools } from '../materialize.js';
import type { LiveTurnProjection } from '../live-turn-projection.js';

const REF = 'maka://runtime/background-tasks/pty-1';
const SESSION = 'session-1';

/**
 * A transcript with background-command history: `turn-1` owns a Bash whose
 * durable revision permanently leads the `tool_result` snapshot persisted in
 * messages. Re-deriving the overlay per token rebuilt that turn on every
 * delta; the projection has to merge it once.
 */
function history(): StoredMessage[] {
  return [
    { type: 'user', id: 'u1', turnId: 'turn-1', ts: 1, text: 'run a job' },
    toolCall('bash-1', 'turn-1', 'Bash', { command: 'job', pty: true }, 2),
    toolResult('bash-1', 'turn-1', shellRun(1), 3),
    { type: 'assistant', id: 'a1', turnId: 'turn-1', ts: 4, text: 'started', modelId: 'model-1' },
    { type: 'user', id: 'u2', turnId: 'turn-2', ts: 5, text: 'and now?' },
    { type: 'assistant', id: 'a2', turnId: 'turn-2', ts: 6, text: 'done', modelId: 'model-1' },
  ];
}

const backgroundUpdate: ShellRunUpdate = {
  sessionId: SESSION,
  ownership: { kind: 'local' },
  sourceTurnId: 'turn-1',
  sourceToolCallId: 'bash-1',
  result: shellRunSnapshot(9),
};

function streamingTurn(text: string): LiveTurnProjection {
  return {
    turnId: 'turn-3',
    phase: 'streamed',
    steps: [{
      stepId: 'step-1',
      contentOrder: ['text'],
      text: { text, truncated: false, complete: false },
      tools: [],
    }],
  };
}

describe('incremental transcript projection', () => {
  test('a plain text delta affects only the tail turn', () => {
    const projection = createTranscriptProjection();
    const messages = history();
    const updates = [backgroundUpdate];
    const first = projection.project({
      sessionId: SESSION,
      messages,
      liveTurn: streamingTurn('he'),
      shellRunUpdates: updates,
    });
    // A first projection has no prior state, so every turn is reported.
    assert.deepEqual([...first.affectedTurnIds].sort(), ['turn-1', 'turn-2', 'turn-3']);

    const second = projection.project({
      sessionId: SESSION,
      messages,
      liveTurn: streamingTurn('hello'),
      shellRunUpdates: updates,
    });
    assert.deepEqual([...second.affectedTurnIds], ['turn-3']);
    assert.strictEqual(second.turns[0], first.turns[0], 'the turn owning the background Bash must keep identity');
    assert.strictEqual(second.turns[1], first.turns[1], 'an unrelated settled turn must keep identity');
    assert.notStrictEqual(second.turns[2], first.turns[2], 'the tail turn carries the delta');

    // The overlay is still applied — identity is preserved, not the merge.
    const bash = second.turns[0]?.tools[0];
    assert.equal(bash?.result?.kind === 'shell_run' ? bash.result.revision : undefined, 9);
  });

  test('a shell-run update whose semantics are unchanged affects nothing', () => {
    const projection = createTranscriptProjection();
    const messages = history();
    const base = { sessionId: SESSION, messages, liveTurn: streamingTurn('he') };
    const settled = projection.project({ ...base, shellRunUpdates: [backgroundUpdate] });

    // A new update object carrying an already-merged revision says nothing new.
    const restated = projection.project({
      ...base,
      shellRunUpdates: [{ ...backgroundUpdate, result: shellRunSnapshot(9) }],
    });
    assert.equal(restated.affectedTurnIds.size, 0, 'a restated revision must produce an empty affected set');
    assert.strictEqual(restated.turns[0], settled.turns[0]);

    const advanced = projection.project({
      ...base,
      shellRunUpdates: [{ ...backgroundUpdate, result: shellRunSnapshot(10) }],
    });
    assert.deepEqual([...advanced.affectedTurnIds], ['turn-1'], 'a real revision advance must affect the owning turn');
  });

  test('a message refresh that adds a turn leaves the other turns identical', () => {
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    // A refresh re-reads the ledger, so every message object is new.
    const after = projection.project({
      sessionId: SESSION,
      messages: [...history(), { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'third' }],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-3']);
    assert.strictEqual(after.turns[0], before.turns[0]);
    assert.strictEqual(after.turns[1], before.turns[1]);
  });

  test('a message refresh that changes one turn affects only that turn', () => {
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    const after = projection.project({
      sessionId: SESSION,
      messages: [
        ...history().slice(0, 5),
        { type: 'assistant', id: 'a2', turnId: 'turn-2', ts: 6, text: 'done, finally', modelId: 'model-1' },
      ],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-2']);
    assert.strictEqual(after.turns[0], before.turns[0]);
    assert.notStrictEqual(after.turns[1], before.turns[1]);
  });

  test('a turn missing from the durable snapshot is dropped, leaving the rest identical', () => {
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    const after = projection.project({ sessionId: SESSION, messages: history().slice(0, 4) });
    assert.deepEqual([...after.affectedTurnIds], ['turn-2']);
    assert.deepEqual(after.turns.map((turn) => turn.turnId), ['turn-1']);
    assert.strictEqual(after.turns[0], before.turns[0]);
  });

  test('deleting an earlier turn keeps the surviving turns identical', () => {
    // Turn order shifts, so identity cannot come from position — a surviving
    // turn is recognised by value, wherever it lands.
    const projection = createTranscriptProjection();
    const before = projection.project({
      sessionId: SESSION,
      messages: [
        ...history(),
        { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'third' },
        { type: 'assistant', id: 'a3', turnId: 'turn-3', ts: 8, text: 'third answer', modelId: 'model-1' },
      ],
    });
    const after = projection.project({
      sessionId: SESSION,
      messages: [
        ...history().slice(0, 4),
        { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'third' },
        { type: 'assistant', id: 'a3', turnId: 'turn-3', ts: 8, text: 'third answer', modelId: 'model-1' },
      ],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-2']);
    assert.deepEqual(after.turns.map((turn) => turn.turnId), ['turn-1', 'turn-3']);
    assert.strictEqual(after.turns[0], before.turns[0]);
    assert.strictEqual(after.turns[1], before.turns[2], 'the turn that moved up keeps identity');
  });

  test('an edited-and-resent prompt invalidates its own turn and nothing before it', () => {
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    const after = projection.project({
      sessionId: SESSION,
      messages: [
        ...history().slice(0, 4),
        { type: 'user', id: 'u2', turnId: 'turn-2', ts: 5, text: 'and now, differently?' },
        { type: 'assistant', id: 'a2', turnId: 'turn-2', ts: 6, text: 'done', modelId: 'model-1' },
      ],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-2']);
    assert.strictEqual(after.turns[0], before.turns[0]);
    assert.equal(after.turns[1]?.user?.text, 'and now, differently?');
  });

  test('a live step handed off to durable messages rebuilds only its own turn', () => {
    const projection = createTranscriptProjection();
    const live = projection.project({
      sessionId: SESSION,
      messages: [...history(), { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'third' }],
      liveTurn: streamingTurn('half an ans'),
    });
    assert.equal(live.turns[2]?.timeline.some((item) => item.kind === 'text' && item.live === true), true);

    // Handoff: the answer lands in messages and the live projection retires.
    const settled = projection.project({
      sessionId: SESSION,
      messages: [
        ...history(),
        { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'third' },
        { type: 'assistant', id: 'step-1', turnId: 'turn-3', ts: 8, text: 'half an answer', modelId: 'model-1' },
      ],
    });
    assert.deepEqual([...settled.affectedTurnIds], ['turn-3'], 'the handoff is not a whole-transcript event');
    assert.strictEqual(settled.turns[0], live.turns[0]);
    assert.strictEqual(settled.turns[1], live.turns[1]);
    assert.notStrictEqual(settled.turns[2], live.turns[2], 'the turn genuinely changed, so its reference must move');
    assert.equal(settled.turns[2]?.assistant?.text, 'half an answer');
    assert.equal(
      settled.turns[2]?.timeline.some((item) => item.kind === 'text' && item.live === true),
      false,
      'the live text must not survive its handoff',
    );
  });

  test('lineage recorded on a turn invalidates that turn', () => {
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    const after = projection.project({
      sessionId: SESSION,
      messages: [
        ...history(),
        { type: 'turn_state', id: 't3', turnId: 'turn-3', ts: 7, status: 'completed', partialOutputRetained: false, regeneratedFromTurnId: 'turn-2' },
        { type: 'user', id: 'u3', turnId: 'turn-3', ts: 7, text: 'again' },
      ],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-3']);
    assert.equal(after.turns[2]?.regeneratedFromTurnId, 'turn-2');
    assert.strictEqual(after.turns[1], before.turns[1]);
  });

  test('an ownership flip on the same revision invalidates the owning turn', () => {
    // revision and ownership are the semantic key — a cache that keyed on the
    // update object alone could hold a stale badge here.
    const projection = createTranscriptProjection();
    const messages = history();
    const owned: ShellRunUpdate = {
      sessionId: SESSION,
      ownership: { kind: 'source_owned', sourceSessionId: 'source', ownerSessionId: 'source' },
      sourceTurnId: 'turn-1',
      sourceToolCallId: 'bash-1',
      result: shellRunSnapshot(9),
    };
    const before = projection.project({ sessionId: SESSION, messages, shellRunUpdates: [owned] });
    assert.equal(before.turns[0]?.tools[0]?.shellRunSource, 'owned');

    const after = projection.project({
      sessionId: SESSION,
      messages,
      shellRunUpdates: [{
        ...owned,
        ownership: { kind: 'source_unavailable', sourceSessionId: 'source' },
        result: shellRunSnapshot(9),
      }],
    });
    assert.deepEqual([...after.affectedTurnIds], ['turn-1']);
    assert.equal(after.turns[0]?.tools[0]?.shellRunSource, 'unavailable');
  });

  test('switching sessions drops the previous session state', () => {
    // Two sessions in one revision lineage can carry the same turn and message
    // ids with different content, so nothing may be reused across a switch.
    const projection = createTranscriptProjection();
    const first = projection.project({ sessionId: SESSION, messages: history() });
    const other = projection.project({
      sessionId: 'session-2',
      messages: [
        ...history().slice(0, 5),
        { type: 'assistant', id: 'a2', turnId: 'turn-2', ts: 6, text: 'a different answer', modelId: 'model-1' },
      ],
    });
    assert.deepEqual([...other.affectedTurnIds].sort(), ['turn-1', 'turn-2']);
    assert.notStrictEqual(other.turns[0], first.turns[0]);
    assert.equal(other.turns[1]?.assistant?.text, 'a different answer');
  });

  test('affects the ShellRun owner turn, not the turn the event names', () => {
    // A WriteStdin in turn-2 carries a shell_run whose `ref` belongs to the
    // Bash in turn-1, and `foldShellRunToolActivities` folds it back into that
    // owner. The turn an event names is therefore NOT the set of turns it
    // affects — which is why the affected set is derived from the projection's
    // own output rather than passed through from the event.
    const projection = createTranscriptProjection();
    const before = projection.project({ sessionId: SESSION, messages: history() });
    const after = projection.project({
      sessionId: SESSION,
      messages: [
        ...history(),
        toolCall('write-1', 'turn-3', 'WriteStdin', { ref: REF, input: 'go\n' }, 7),
        toolResult('write-1', 'turn-3', shellRun(4), 8),
      ],
    });
    assert.deepEqual([...after.affectedTurnIds].sort(), ['turn-1', 'turn-3']);
    const owner = after.turns[0]?.tools[0];
    assert.equal(owner?.toolName, 'Bash');
    assert.equal(owner?.result?.kind === 'shell_run' ? owner.result.revision : undefined, 4);
    assert.strictEqual(after.turns[1], before.turns[1], 'the untouched turn keeps identity');
  });

  test('applies an update whose only target is a live-only tool', () => {
    // A durable update can arrive before the Bash tool_call is persisted, so
    // the canonical settled tool map has no target for it yet. The overlay runs
    // over the live-merged turns, where the live-only tool already exists.
    const projection = createTranscriptProjection();
    const { turns, affectedTurnIds } = projection.project({
      sessionId: SESSION,
      messages: [],
      liveTurn: {
        turnId: 'turn-live',
        phase: 'streamed',
        steps: [{
          stepId: 'tool:bash-live',
          contentOrder: ['tools'],
          tools: [{ toolUseId: 'bash-live', toolName: 'Bash', status: 'running', args: { command: 'job', pty: true } }],
        }],
      },
      shellRunUpdates: [{
        sessionId: SESSION,
        ownership: { kind: 'source_owned', sourceSessionId: 'source', ownerSessionId: 'source' },
        sourceTurnId: 'turn-live',
        sourceToolCallId: 'bash-live',
        result: shellRunSnapshot(2),
      }],
    });
    assert.deepEqual([...affectedTurnIds], ['turn-live']);
    const tool = turns[0]?.tools[0];
    assert.equal(tool?.result?.kind, 'shell_run');
    assert.equal(tool?.shellRunSource, 'owned');
  });

  test('projecting identical inputs twice returns the same result object', () => {
    const projection = createTranscriptProjection();
    const input = { sessionId: SESSION, messages: history(), liveTurn: streamingTurn('he') };
    const first = projection.project(input);
    assert.strictEqual(projection.project(input), first);
  });

  test('turn.tools is exactly the tools its timeline renders', () => {
    const projection = createTranscriptProjection();
    const { turns } = projection.project({
      sessionId: SESSION,
      messages: history(),
      liveTurn: {
        turnId: 'turn-3',
        phase: 'streamed',
        steps: [{
          stepId: 'step-1',
          contentOrder: ['tools'],
          tools: [{ toolUseId: 'live-1', toolName: 'Read', status: 'running', args: {} }],
        }],
      },
      shellRunUpdates: [backgroundUpdate],
    });
    for (const turn of turns) {
      assert.deepEqual(turn.tools, timelineTools(turn.timeline), `turn ${turn.turnId}`);
    }
    assert.deepEqual(turns[2]?.tools.map((tool) => tool.toolUseId), ['live-1']);
  });
});

function toolCall(
  id: string,
  turnId: string,
  toolName: string,
  args: unknown,
  ts: number,
): Extract<StoredMessage, { type: 'tool_call' }> {
  return { type: 'tool_call', id, turnId, ts, toolName, args };
}

function toolResult(
  toolUseId: string,
  turnId: string,
  content: ShellRunToolResult,
  ts: number,
): Extract<StoredMessage, { type: 'tool_result' }> {
  return { type: 'tool_result', id: `result-${toolUseId}`, turnId, ts, toolUseId, isError: false, content };
}

function shellRun(revision: number): Extract<ShellRunToolResult, { mode: 'pty' }> {
  return {
    kind: 'shell_run',
    ref: REF,
    mode: 'pty',
    status: 'running',
    cwd: '/repo',
    cmd: 'job',
    startedAt: 1,
    updatedAt: revision,
    revision,
    output: {
      mode: 'pty',
      screen: 'ready',
      scrollback: '',
      cols: 80,
      rows: 24,
      cursor: { x: 5, y: 0, visible: true },
      alternateScreen: false,
      truncated: false,
      redacted: false,
    },
  };
}

function shellRunSnapshot(revision: number): Extract<ShellRunSnapshotResult, { mode: 'pty' }> {
  return {
    kind: 'shell_run',
    ref: REF,
    mode: 'pty',
    status: 'running',
    cwd: '/repo',
    cmd: 'job',
    startedAt: 1,
    updatedAt: revision,
    revision,
    output: {
      mode: 'pty',
      screen: 'ready',
      scrollback: '',
      cols: 80,
      rows: 24,
      cursor: { x: 5, y: 0, visible: true },
      alternateScreen: false,
      truncated: false,
      redacted: false,
    },
  };
}
