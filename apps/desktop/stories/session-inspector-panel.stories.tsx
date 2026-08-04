import type { Meta, StoryObj } from '@storybook/react-vite';
import type { SessionTrace } from '@maka/core/session-trace';
import { Section } from '@astryxdesign/core/Section';
import { SessionInspectorPanel } from '../src/renderer/session-inspector-panel';
import { withScopedMakaBridge } from './maka-bridge';

// Fidelity convention (#1433): every story below names the real app path
// that reaches it. See apps/desktop/stories/FIDELITY.md.
//
// App path: 会话工作栏 → 追踪. The panel is the workbar's least-covered tab and
// its states are read-only projections of one IPC call, so the bridge below is
// the whole world it needs.

const NOW = Date.UTC(2026, 6, 31, 10, 30, 0);

const trace: SessionTrace = {
  schemaVersion: 1,
  sessionId: 'session-trace',
  turns: [
    {
      turnId: 'turn-1',
      runId: 'run-1',
      startedAt: NOW,
      endedAt: NOW + 8_400,
      durationMs: 8_400,
      steps: [
        {
          kind: 'model_call',
          id: 'call-1',
          turnId: 'turn-1',
          runId: 'run-1',
          startedAt: NOW,
          endedAt: NOW + 3_100,
          durationMs: 3_100,
          callKind: 'main',
          providerId: 'zhipu',
          modelId: 'glm-5.1',
          step: 0,
          status: 'completed',
          costUsd: 0.0182,
          attempts: [
            {
              attemptId: 'attempt-1a',
              attempt: 0,
              status: 'failed',
              startedAt: NOW,
              completedAt: NOW + 900,
              latencyMs: 900,
              errorClass: 'overloaded',
              costBasis: 'unpriced',
              usageBasis: 'missing',
            },
            {
              attemptId: 'attempt-1b',
              attempt: 1,
              status: 'completed',
              startedAt: NOW + 1_000,
              completedAt: NOW + 3_100,
              latencyMs: 2_100,
              inputTokens: 6_200,
              outputTokens: 480,
              costUsd: 0.0182,
              costBasis: 'priced',
              usageBasis: 'reported',
            },
          ],
        },
        {
          kind: 'tool',
          id: 'tool-1',
          turnId: 'turn-1',
          runId: 'run-1',
          startedAt: NOW + 3_200,
          endedAt: NOW + 3_760,
          durationMs: 560,
          toolName: 'read',
          status: 'completed',
        },
        {
          kind: 'compaction',
          id: 'compaction-1',
          turnId: 'turn-1',
          runId: 'run-1',
          startedAt: NOW + 3_800,
          checkpointId: 'checkpoint-9',
        },
      ],
      totals: {
        durationMs: 8_400,
        modelAttempts: 2,
        retries: 1,
        compactions: 1,
        inputTokens: 6_200,
        outputTokens: 480,
        costUsd: 0.0182,
        unpricedAttempts: 1,
      },
    },
    {
      turnId: 'turn-2',
      runId: 'run-2',
      startedAt: NOW + 20_000,
      endedAt: NOW + 24_500,
      durationMs: 4_500,
      steps: [
        {
          kind: 'tool',
          id: 'tool-2',
          turnId: 'turn-2',
          runId: 'run-2',
          startedAt: NOW + 20_100,
          endedAt: NOW + 21_900,
          durationMs: 1_800,
          toolName: 'bash',
          status: 'failed',
          recovered: { disposition: 'parked', reasonCode: 'sandbox_denied' },
        },
        {
          kind: 'error',
          id: 'error-1',
          turnId: 'turn-2',
          runId: 'run-2',
          startedAt: NOW + 22_000,
          message: 'sandbox denied the write',
        },
      ],
      totals: {
        durationMs: 4_500,
        modelAttempts: 0,
        retries: 0,
        compactions: 0,
        inputTokens: 0,
        outputTokens: 0,
        unpricedAttempts: 0,
      },
      failure: { code: 'tool_failed', message: 'sandbox denied the write' },
    },
  ],
  totals: {
    durationMs: 12_900,
    modelAttempts: 2,
    retries: 1,
    compactions: 1,
    inputTokens: 6_200,
    outputTokens: 480,
    costUsd: 0.0182,
    unpricedAttempts: 1,
  },
  coverage: {
    modelCalls: 'partial',
    turnsMissingModelCalls: ['turn-2'],
    unreadableRecords: 1,
    turnsWithFewerModelCallsThanSteps: [],
  },
};

const emptyTrace: SessionTrace = {
  schemaVersion: 1,
  sessionId: 'session-trace',
  turns: [],
  totals: {
    durationMs: 0,
    modelAttempts: 0,
    retries: 0,
    compactions: 0,
    inputTokens: 0,
    outputTokens: 0,
    unpricedAttempts: 0,
  },
  coverage: {
    modelCalls: 'none',
    turnsMissingModelCalls: [],
    unreadableRecords: 0,
    turnsWithFewerModelCallsThanSteps: [],
  },
};

const noEvents = { sessions: { subscribeEvents: () => () => {} } };

function bridge(result: unknown) {
  return withScopedMakaBridge({ ...noEvents, inspector: { trace: async () => result } });
}

const meta: Meta<typeof SessionInspectorPanel> = {
  title: 'Product/Session Trace',
  component: SessionInspectorPanel,
  parameters: { layout: 'fullscreen' },
  args: { sessionId: 'session-trace', active: true },
  decorators: [
    // The host, written out because it cannot be imported: `WorkbarPanel` is
    // private to session-workbar.tsx. What the panel takes from it is the
    // bounded 1fr grid track plus `.maka-session-workbar-panel`
    // (`min-height: 0; overflow: hidden`) — reproduced here class for class, at
    // the workbar's default 400px. Keep in step with WorkbarPanel.
    (Story) => (
      <div style={{ width: 400, height: 640, display: 'grid' }}>
        <Section variant="transparent" padding={0} className="maka-session-workbar-panel">
          <Story />
        </Section>
      </div>
    ),
  ],
};
export default meta;

type Story = StoryObj<typeof SessionInspectorPanel>;

// Real path: 会话工作栏 → 追踪, on a session that has run at least one turn —
// the retry, compaction and failed turn below are what a real trace of a
// rate-limited, sandbox-denied session projects to.
export const Timeline: Story = {
  decorators: [bridge({ ok: true, data: trace })],
};

// Real path: 会话工作栏 → 追踪 on a session that has not run a turn yet — the
// state the task-ledger e2e fixture opens on.
export const Empty: Story = {
  decorators: [bridge({ ok: true, data: emptyTrace })],
};

// Real path: 会话工作栏 → 追踪 when `inspector.trace` rejects the read (an
// unreadable or partially written run ledger); retry lives on the banner.
export const ReadFailed: Story = {
  decorators: [bridge({ ok: false, error: { message: '追踪读取失败：无法读取运行记录' } })],
};
