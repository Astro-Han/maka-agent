import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { LanguageModelV4CallOptions, LanguageModelV4StreamPart } from '@ai-sdk/provider';
import type { RuntimeExecutionConnection } from '@maka/core';
import { getAIModel } from '@maka/runtime';

/**
 * Regression guard for #1967 and #1976. Both are the same modelling defect: the streamed
 * `tool_calls[].index` is an association label a gateway may omit, repeat, or number
 * freely, and it was being used as the storage slot, the identity, and the ordering all at
 * once. Identity actually lives in `id`; `index` only aliases it for argument deltas that
 * carry no `id`.
 *
 * #1967 — index as a storage slot. Anthropic→OpenAI translators reuse the Anthropic
 * content-block index, so the first tool call arrives as index 1 once a text block
 * consumed index 0, leaving a hole that crashed the flush.
 *
 * #1976 — index as identity. Ollama labels every tool call in a turn with index 0
 * (vercel/ai#14277), which merged distinct calls into one: arguments concatenated into
 * invalid JSON, the second `id` and `name` dropped, and no error anywhere. Also covered
 * here: deltas that omit `index` entirely, which used to pick a fresh slot per chunk and
 * throw `Expected 'id' to be a string.`
 */

const connection: RuntimeExecutionConnection = {
  slug: 'relay',
  providerType: 'openai-compatible',
  baseUrl: 'https://relay.invalid/v1',
  defaultModel: 'claude-opus-4-8',
};

const prompt: LanguageModelV4CallOptions['prompt'] = [
  { role: 'user', content: [{ type: 'text', text: 'read a.txt' }] },
];

const tools: LanguageModelV4CallOptions['tools'] = [
  {
    type: 'function',
    name: 'read_file',
    inputSchema: { type: 'object', properties: { path: { type: 'string' } } },
  },
];

function chunk(delta: unknown, finishReason: string | null = null): unknown {
  return {
    id: 'chatcmpl-1',
    object: 'chat.completion.chunk',
    created: 0,
    model: 'claude-opus-4-8',
    choices: [{ index: 0, delta, finish_reason: finishReason }],
  };
}

interface StreamedToolCall {
  index: number;
  id: string;
  path: string;
}

/** A gateway turn that emits one text block, then the given tool calls. */
function streamingRelay(toolCalls: StreamedToolCall[]): typeof globalThis.fetch {
  const payloads = [
    chunk({ role: 'assistant', content: 'Reading it.' }),
    ...toolCalls.flatMap(({ index, id, path }) => [
      chunk({
        tool_calls: [
          { index, id, type: 'function', function: { name: 'read_file', arguments: '' } },
        ],
      }),
      chunk({ tool_calls: [{ index, function: { arguments: `{"path":"${path}"}` } }] }),
    ]),
    chunk({}, 'tool_calls'),
  ];
  const body = `${payloads.map((payload) => `data: ${JSON.stringify(payload)}\n\n`).join('')}data: [DONE]\n\n`;
  return async () =>
    new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

async function collectStream(toolCalls: StreamedToolCall[]): Promise<LanguageModelV4StreamPart[]> {
  const model = getAIModel({
    connection,
    apiKey: 'test-key',
    modelId: 'claude-opus-4-8',
    fetch: streamingRelay(toolCalls),
  });
  const { stream } = await model.doStream({ prompt, tools });
  const parts: LanguageModelV4StreamPart[] = [];
  for await (const part of stream) parts.push(part);
  return parts;
}

function toolCallsOf(parts: LanguageModelV4StreamPart[]) {
  return parts
    .filter((part) => part.type === 'tool-call')
    .map(({ toolCallId, toolName, input }) => ({ toolCallId, toolName, input }));
}

function assertStreamSucceeded(parts: LanguageModelV4StreamPart[]): void {
  assert.equal(
    parts.at(-1)?.type,
    'finish',
    'the stream must close cleanly instead of failing the whole turn',
  );
  assert.deepEqual(
    parts.filter((part) => part.type === 'error'),
    [],
    'a non-zero tool call index is not a stream error',
  );
}

describe('getAIModel: OpenAI-compatible streamed tool_calls index', () => {
  for (const index of [0, 1, 7]) {
    test(`emits the tool call when the gateway labels it index ${index}`, async () => {
      const parts = await collectStream([{ index, id: 'call_1', path: 'a.txt' }]);

      assert.deepEqual(toolCallsOf(parts), [
        { toolCallId: 'call_1', toolName: 'read_file', input: '{"path":"a.txt"}' },
      ]);
      assertStreamSucceeded(parts);
    });
  }

  // Holes must not merge, drop, or reorder calls either. A fix that appends every
  // new index instead of honouring it would still pass the single-call cases above.
  test('keeps two tool calls distinct and ordered when index 0 is a hole', async () => {
    const parts = await collectStream([
      { index: 1, id: 'call_1', path: 'a.txt' },
      { index: 2, id: 'call_2', path: 'b.txt' },
    ]);

    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_1', toolName: 'read_file', input: '{"path":"a.txt"}' },
      { toolCallId: 'call_2', toolName: 'read_file', input: '{"path":"b.txt"}' },
    ]);
    assertStreamSucceeded(parts);
  });
});

/**
 * A streamed `tool_calls[]` delta exactly as a gateway may put it on the wire, including
 * the shapes the OpenAI protocol does not allow. The tests below drive these directly
 * because the defect lives in how deltas associate, which `streamingRelay`'s well-formed
 * shape cannot express.
 */
interface ToolCallDelta {
  index?: number;
  id?: string;
  type?: 'function';
  function?: { name?: string; arguments?: string };
}

const twoTools: LanguageModelV4CallOptions['tools'] = [
  {
    type: 'function',
    name: 'read_file',
    inputSchema: { type: 'object', properties: { path: { type: 'string' } } },
  },
  {
    type: 'function',
    name: 'write_file',
    inputSchema: { type: 'object', properties: { path: { type: 'string' } } },
  },
];

/** A gateway turn that emits each delta in its own chunk, then finishes on `tool_calls`. */
function deltaRelay(deltas: readonly ToolCallDelta[]): typeof globalThis.fetch {
  const payloads = [
    ...deltas.map((delta) => chunk({ tool_calls: [delta] })),
    chunk({}, 'tool_calls'),
  ];
  const body = `${payloads.map((payload) => `data: ${JSON.stringify(payload)}\n\n`).join('')}data: [DONE]\n\n`;
  return async () =>
    new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

/**
 * Collects a turn, capturing a mid-stream throw instead of failing the test on it.
 *
 * `providerType` matters because `openai` and `openai-compatible` construct the same
 * `StreamingToolCallTracker` but reach it differently: the compatible adapter keeps its own
 * index-keyed buffer in front, and its chunk schema allows an absent `index`, while
 * `openai`'s requires one. A defect in the shared tracker can therefore surface on one path
 * and not the other, so the shapes both paths can express are checked on both.
 */
async function collectDeltas(
  deltas: readonly ToolCallDelta[],
  providerType: 'openai-compatible' | 'openai' = 'openai-compatible',
): Promise<{ parts: LanguageModelV4StreamPart[]; failure: unknown }> {
  const model = getAIModel({
    connection: { ...connection, providerType },
    apiKey: 'test-key',
    modelId: 'claude-opus-4-8',
    fetch: deltaRelay(deltas),
  });
  const { stream } = await model.doStream({ prompt, tools: twoTools });
  const parts: LanguageModelV4StreamPart[] = [];
  try {
    for await (const part of stream) parts.push(part);
  } catch (error) {
    return { parts, failure: error };
  }
  const errorPart = parts.find((part) => part.type === 'error');
  return { parts, failure: errorPart ? (errorPart as { error: unknown }).error : undefined };
}

/**
 * The property that must hold for every shape in this file: an emitted input is either empty
 * or parses on its own. Splicing two calls' arguments together always breaks this, including
 * when the fragments interleave (`{"path"{"path":"a"}:"b"}`) rather than append cleanly, which
 * a substring check for `}{` would miss. Asserted as a property so the file keeps catching
 * the defect class rather than only the exact outputs these cases happen to produce.
 */
function assertInputsSelfContained(parts: LanguageModelV4StreamPart[]): void {
  for (const { toolCallId, input } of toolCallsOf(parts)) {
    if (input === '') continue;
    try {
      JSON.parse(input);
    } catch {
      assert.fail(`tool call ${JSON.stringify(toolCallId)} input does not parse alone: ${input}`);
    }
  }
}

describe('getAIModel: streamed tool call identity is `id`, not `index`', () => {
  // The Ollama shape (vercel/ai#14277): every call in the turn is labelled index 0, each
  // arriving complete in one delta. This is the case that silently produced one call with
  // concatenated invalid JSON arguments.
  test('keeps two calls distinct when a gateway labels both index 0', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path":"a"}' },
      },
      {
        index: 0,
        id: 'call_b',
        type: 'function',
        function: { name: 'write_file', arguments: '{"path":"b"}' },
      },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
      { toolCallId: 'call_b', toolName: 'write_file', input: '{"path":"b"}' },
    ]);
  });

  // The second call's `name` must survive too. Merging dropped it, so the second call ran
  // under the first call's tool name whenever the concatenation happened to stay parsable.
  test('keeps each call name when a reused index spreads arguments over chunks', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path"' },
      },
      { index: 0, function: { arguments: ':"a"}' } },
      {
        index: 0,
        id: 'call_b',
        type: 'function',
        function: { name: 'write_file', arguments: '{"path"' },
      },
      { index: 0, function: { arguments: ':"b"}' } },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
      { toolCallId: 'call_b', toolName: 'write_file', input: '{"path":"b"}' },
    ]);
  });

  // The worst shape: the merge stays valid JSON, so nothing downstream can notice. The
  // second call used to vanish with no error, no log, and a successful-looking turn.
  test('does not swallow a reused-index call whose arguments are empty', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path":"a"}' },
      },
      { index: 0, id: 'call_b', type: 'function', function: { name: 'write_file', arguments: '' } },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
      { toolCallId: 'call_b', toolName: 'write_file', input: '' },
    ]);
  });

  // Once an index has been reused, an argument-only delta belongs to the call that most
  // recently claimed that index — the new one, not the one it displaced.
  test('routes an index-only continuation to the call that last claimed the index', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path":"a"}' },
      },
      {
        index: 0,
        id: 'call_b',
        type: 'function',
        function: { name: 'write_file', arguments: '{"path"' },
      },
      { index: 0, function: { arguments: ':"b"}' } },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
      { toolCallId: 'call_b', toolName: 'write_file', input: '{"path":"b"}' },
    ]);
  });

  // Deltas with no `index` at all used to pick a fresh slot per chunk via the
  // `?? toolCalls.length` fallback and throw `Expected 'id' to be a string.`
  test('accumulates a call whose deltas omit index entirely', async () => {
    const { parts, failure } = await collectDeltas([
      { id: 'call_a', type: 'function', function: { name: 'read_file', arguments: '{"pa' } },
      { function: { arguments: 'th":"a"}' } },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
    ]);
  });

  // A delta with neither field can only be attributed when there is nothing to confuse it
  // with. Guessing a target — "the last created call", "the call the stream last touched" —
  // is how one call's arguments end up on another, which is the defect this file exists for.
  test('refuses to guess a target for a bare delta while several calls are open', async () => {
    const { parts, failure } = await collectDeltas([
      { index: 0, id: 'call_a', type: 'function', function: { name: 'read_file', arguments: '' } },
      {
        index: 1,
        id: 'call_b',
        type: 'function',
        function: { name: 'write_file', arguments: '{}' },
      },
      { function: { arguments: '{"path":"a"}' } },
    ]);

    assert.notEqual(failure, undefined, 'an unattributable delta must not be absorbed');
    assertInputsSelfContained(parts);
  });

  // When every call has its own index, that index is a usable final position and ordering
  // must follow it rather than arrival — a gateway may stream a later slot first.
  test('emits in index order when out-of-order indices are unique', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 2,
        id: 'call_second',
        type: 'function',
        function: { name: 'write_file', arguments: '{}' },
      },
      {
        index: 1,
        id: 'call_first',
        type: 'function',
        function: { name: 'read_file', arguments: '{}' },
      },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(
      toolCallsOf(parts).map(({ toolCallId }) => toolCallId),
      ['call_first', 'call_second'],
    );
  });

  // A repeated index carries no ordering information, so arrival order is all that is
  // left. Sorting by it anyway would let a malformed index reorder the turn.
  test('emits in arrival order when the index is reused and cannot order anything', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_first',
        type: 'function',
        function: { name: 'read_file', arguments: '{}' },
      },
      {
        index: 0,
        id: 'call_second',
        type: 'function',
        function: { name: 'write_file', arguments: '{}' },
      },
    ]);

    assert.equal(failure, undefined);
    assert.deepEqual(
      toolCallsOf(parts).map(({ toolCallId }) => toolCallId),
      ['call_first', 'call_second'],
    );
  });

  /**
   * Known boundary, deliberately locked in. `@ai-sdk/openai-compatible` keeps its own
   * index-keyed buffer in front of the tracker and forwards a delta as soon as it has a
   * `name`; once an index has been forwarded, later deltas on it bypass that buffer. So a
   * reused index whose new call has not sent its `name` yet reaches the tracker as an
   * unnamed new identity and the turn fails.
   *
   * Failing is the point: the old behaviour appended those arguments to the previous call.
   * Closing this properly means folding the adapter's buffer into the tracker so a call
   * can stay pending until its name arrives, which is a rewrite of both layers rather than
   * this fix. What must never come back is the silent merge.
   */
  test('fails loudly rather than merging when a reused index delays its name', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path":"a"}' },
      },
      { index: 0, id: 'call_b', type: 'function', function: { arguments: '{"path"' } },
      { index: 0, function: { name: 'write_file', arguments: ':"b"}' } },
    ]);

    assert.notEqual(failure, undefined, 'an unnamed new identity must not pass silently');
    assertInputsSelfContained(parts);
  });

  // `id` is no more trustworthy than `index` was. A gateway that repeats one across distinct
  // calls must not have them merged — the same defect as the reused index, with the two
  // fields swapped. The index disagrees here, and that disagreement is what has to win.
  for (const id of ['', 'dup']) {
    test(`keeps calls distinct when both reuse id ${JSON.stringify(id)} at different indices`, async () => {
      const { parts, failure } = await collectDeltas([
        { index: 0, id, type: 'function', function: { name: 'read_file', arguments: '{"path"' } },
        { index: 1, id, type: 'function', function: { name: 'write_file', arguments: '{"path"' } },
        { index: 0, function: { arguments: ':"a"}' } },
        { index: 1, function: { arguments: ':"b"}' } },
      ]);

      assert.equal(failure, undefined);
      assertInputsSelfContained(parts);
      assert.deepEqual(
        toolCallsOf(parts).map(({ toolName, input }) => ({ toolName, input })),
        [
          { toolName: 'read_file', input: '{"path":"a"}' },
          { toolName: 'write_file', input: '{"path":"b"}' },
        ],
      );
    });
  }

  // Some gateways send `id: ''` on continuation deltas instead of omitting the field. Read
  // literally, that is a new identity with no name, which kills the whole turn.
  test('treats an empty id on a continuation as absent rather than a new call', async () => {
    const { parts, failure } = await collectDeltas([
      {
        index: 0,
        id: 'call_a',
        type: 'function',
        function: { name: 'read_file', arguments: '{"path"' },
      },
      { index: 0, id: '', function: { arguments: ':"a"}' } },
    ]);

    assert.equal(failure, undefined, 'an empty id must not be read as a new identity');
    assertInputsSelfContained(parts);
    assert.deepEqual(toolCallsOf(parts), [
      { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
    ]);
  });

  // The `openai` provider builds the same tracker with no buffer in front of it. Its chunk
  // schema requires `index`, so it cannot express the index-omitted shapes, but it can
  // express both reuse defects — and it is the path with no other coverage in this file.
  describe('the openai provider shares this tracker', () => {
    test('keeps two calls distinct when both are labelled index 0', async () => {
      const { parts, failure } = await collectDeltas(
        [
          {
            index: 0,
            id: 'call_a',
            type: 'function',
            function: { name: 'read_file', arguments: '{"path":"a"}' },
          },
          {
            index: 0,
            id: 'call_b',
            type: 'function',
            function: { name: 'write_file', arguments: '{"path":"b"}' },
          },
        ],
        'openai',
      );

      assert.equal(failure, undefined);
      assertInputsSelfContained(parts);
      assert.deepEqual(toolCallsOf(parts), [
        { toolCallId: 'call_a', toolName: 'read_file', input: '{"path":"a"}' },
        { toolCallId: 'call_b', toolName: 'write_file', input: '{"path":"b"}' },
      ]);
    });

    test('keeps two calls distinct when both reuse one id at different indices', async () => {
      const { parts, failure } = await collectDeltas(
        [
          {
            index: 0,
            id: 'dup',
            type: 'function',
            function: { name: 'read_file', arguments: '{"path":"a"}' },
          },
          {
            index: 1,
            id: 'dup',
            type: 'function',
            function: { name: 'write_file', arguments: '{"path":"b"}' },
          },
        ],
        'openai',
      );

      assert.equal(failure, undefined);
      assertInputsSelfContained(parts);
      assert.deepEqual(
        toolCallsOf(parts).map(({ toolName, input }) => ({ toolName, input })),
        [
          { toolName: 'read_file', input: '{"path":"a"}' },
          { toolName: 'write_file', input: '{"path":"b"}' },
        ],
      );
    });
  });
});
