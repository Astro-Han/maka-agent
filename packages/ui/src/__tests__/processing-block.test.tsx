import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import { createElement, type ReactNode } from 'react';
import { renderToStaticMarkup as renderReactToStaticMarkup } from 'react-dom/server';
import { LocaleProvider } from '../locale-context.js';
import type { ToolActivityItem, TurnViewModel } from '../materialize.js';
import { TurnView } from '../chat-turn.js';

function renderToStaticMarkup(node: ReactNode): string {
  return renderReactToStaticMarkup(createElement(LocaleProvider, {
    locale: 'zh',
    children: node,
  }));
}

function renderInDeterministicFixture(node: ReactNode): string {
  const documentDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'document');
  Object.defineProperty(globalThis, 'document', {
    configurable: true,
    value: {
      documentElement: {
        dataset: { makaE2eFixture: 'true' },
      },
    },
  });
  try {
    return renderToStaticMarkup(node);
  } finally {
    if (documentDescriptor) {
      Object.defineProperty(globalThis, 'document', documentDescriptor);
    } else {
      Reflect.deleteProperty(globalThis, 'document');
    }
  }
}

function turnWithTools(tools: ToolActivityItem[]): TurnViewModel {
  return {
    turnId: 'turn-1',
    status: 'completed',
    partialOutputRetained: false,
    tools,
    notes: [],
    timeline: [
      { kind: 'thinking', text: 'reasoning', messageId: 'a1' },
      { kind: 'tools', items: tools },
    ],
    startedAt: 1,
  };
}

describe('ProcessingBlock disclosure wiring (#1307)', () => {
  it('does not wrap native reasoning and tool disclosures in another processing layer', () => {
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: turnWithTools([
        { toolUseId: 'r1', toolName: 'Read', activityKind: 'read', status: 'completed', args: {} },
      ]),
    }));

    assert.doesNotMatch(markup, /data-processing="block"/);
    assert.match(markup, /class="[^"]*astryx-chat-tool-calls[^"]*"/);
    assert.match(markup, /深度思考/);
  });

  it('keeps an unsettled group in the same Astryx rows, expanded, without a processing wrapper', () => {
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: turnWithTools([
        { toolUseId: 'b1', toolName: 'Bash', activityKind: 'command', status: 'completed', args: {} },
        { toolUseId: 'w1', toolName: 'Write', activityKind: 'edit', status: 'running', args: {}, intent: '写入配置' },
      ]),
    }));
    assert.doesNotMatch(markup, /data-processing="block"/);
    assert.match(markup, /class="[^"]*astryx-chat-tool-calls[^"]*"/);
    assert.match(markup, /aria-expanded="true"/);
    assert.match(markup, /写入配置/);
  });
});

describe('deep-thinking disclosure', () => {
  it('shows complete live reasoning immediately in deterministic fixtures', () => {
    const markup = renderInDeterministicFixture(createElement(TurnView, {
      turn: {
        turnId: 'fixture-thinking-turn',
        status: 'running',
        partialOutputRetained: false,
        tools: [],
        notes: [],
        timeline: [{
          kind: 'thinking',
          text: 'deterministic fixture reasoning',
          messageId: 'fixture-thinking-1',
          live: true,
        }],
        startedAt: 1,
      },
      liveStreaming: {},
    }));

    assert.match(markup, /deterministic fixture reasoning/);
  });

  it('renders through official Astryx ChatReasoning with its native collapsed preview', () => {
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: {
        turnId: 'thinking-turn',
        status: 'completed',
        partialOutputRetained: false,
        tools: [],
        notes: [],
        timeline: [{ kind: 'thinking', text: 'private reasoning', messageId: 'thinking-1' }],
        startedAt: 1,
      },
    }));

    assert.match(markup, /class="[^"]*astryx-chat-reasoning[^"]*"/);
    assert.match(markup, /class="maka-assistant-answer-content"/);
    assert.match(markup, /role="button"[^>]*aria-expanded="false"/);
    // The reasoning body carries the product wrap class so
    // `.maka-chat-reasoning-content` in styles.css can restore pre-wrap —
    // Astryx's own atoms declare no white-space, so without this seam the
    // inherited `normal` collapses every newline in the thinking text.
    assert.match(markup, /class="maka-chat-reasoning-content [^"]*"/);
    assert.match(markup, /private reasoning/);
    assert.doesNotMatch(markup, /data-slot="reasoning-trigger"/);
    assert.doesNotMatch(markup, /复制思考过程/);
    assert.doesNotMatch(markup, /data-slot="reasoning-disclosure"/);
  });

  it('draws its chevron from the same Astryx icon registry as the tool rows', () => {
    // One chevron authority. The ejected lab component used to hand-write its
    // own 12-viewBox chevron at strokeWidth 1.5; the tool rows use Astryx
    // `Icon icon="chevronDown"`, a 24-viewBox glyph at the theme's 1.75.
    // chat-message.css forces both to 10x10, so the two identical-looking
    // glyphs rendered at 1.25px and 0.73px of stroke — the reasoning chevron
    // read visibly heavier.
    // Comparing the rendered chevron markup pins the shared source: any
    // divergence in viewBox, stroke width, or size class fails here.
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: {
        ...turnWithTools([
          { toolUseId: 'r1', toolName: 'Read', activityKind: 'read', status: 'completed', args: {} },
          { toolUseId: 'r2', toolName: 'Read', activityKind: 'read', status: 'completed', args: {} },
        ]),
        timeline: [
          { kind: 'thinking', text: 'private reasoning', messageId: 'a1' },
          {
            kind: 'tools',
            items: [
              { toolUseId: 'r1', toolName: 'Read', activityKind: 'read', status: 'completed', args: {} },
              { toolUseId: 'r2', toolName: 'Read', activityKind: 'read', status: 'completed', args: {} },
            ],
          },
        ],
      },
    }));

    const icons = markup.match(/<span[^>]*class="astryx-icon[^"]*"[^>]*>.*?<\/svg><\/span>/g) ?? [];
    const chevrons = icons.filter((icon) => icon.includes('M6 9l6 6 6-6'));
    assert.ok(chevrons.length >= 2, `expected reasoning + tool chevrons, got ${chevrons.length}`);
    assert.equal(new Set(chevrons).size, 1, 'reasoning and tool chevrons must render identical markup');
    assert.match(chevrons[0]!, /data-size="xsm"/);
    assert.match(chevrons[0]!, /viewBox="0 0 24 24"/);
    assert.doesNotMatch(markup, /viewBox="0 0 12 12"/);
  });

  it('places assistant actions on ChatMessageMetadata, not a product Marker footer shell', () => {
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: {
        turnId: 'footer-turn',
        status: 'completed',
        partialOutputRetained: false,
        tools: [],
        notes: [],
        timeline: [{ kind: 'text', text: 'answer body', messageId: 'a1', ts: 1 }],
        startedAt: 1,
        assistant: { id: 'a1', role: 'assistant', text: 'answer body', ts: 1 },
      },
      footerActions: [
        { id: 'copy', label: '复制', enabled: true },
        { id: 'regenerate', label: '重新生成', enabled: true },
      ],
    }));

    assert.match(markup, /astryx-chat-message-metadata|chat-message-metadata/);
    assert.match(markup, /data-action="copy"/);
    assert.match(markup, /data-action="regenerate"/);
    assert.doesNotMatch(markup, /data-variant="footer"/);
  });

  it('keeps redaction and truncation product behavior outside the Astryx disclosure', () => {
    const secret = 'sk-abcdefghijklmnopqrstuvwxyz123456';
    const markup = renderToStaticMarkup(createElement(TurnView, {
      turn: {
        turnId: 'safe-thinking-turn',
        status: 'completed',
        partialOutputRetained: false,
        tools: [],
        notes: [],
        timeline: [{
          kind: 'thinking',
          text: `api_key=${secret}`,
          truncated: true,
          messageId: 'thinking-2',
        }],
        startedAt: 1,
      },
    }));

    assert.match(markup, /深度思考 · 已截断/);
    assert.match(markup, /&lt;redacted&gt;/);
    assert.doesNotMatch(markup, new RegExp(secret));
    assert.doesNotMatch(markup, /data-truncated="true"/);
  });
});
