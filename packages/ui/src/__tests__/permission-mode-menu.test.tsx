import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import type { PermissionMode, UiLocale } from '@maka/core';
import { PERMISSION_MODES } from '@maka/core';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { getConversationCopy } from '../conversation-copy.js';
import { LocaleProvider } from '../locale-context.js';
import { PermissionModeSelect, getPermissionModeMeta } from '../permission-mode-menu.js';

function render(locale: UiLocale, mode: PermissionMode): string {
  return renderToStaticMarkup(
    <LocaleProvider locale={locale}>
      <PermissionModeSelect activeMode={mode} onSelect={() => {}} />
    </LocaleProvider>,
  );
}

describe('Permission mode surface', () => {
  it('shows a read-only session as read-only, with its own hint (#1611)', () => {
    for (const locale of ['zh', 'en'] as const) {
      const meta = getPermissionModeMeta(locale);
      const markup = render(locale, 'explore');

      assert.match(markup, new RegExp(escapeRegExp(escapeMarkup(meta.explore.label))));
      assert.match(markup, new RegExp(escapeRegExp(escapeMarkup(meta.explore.hint))));
      assert.doesNotMatch(markup, new RegExp(escapeRegExp(escapeMarkup(meta.ask.hint))));
    }
  });

  it('keeps legacy execute sessions collapsed to Auto', () => {
    for (const locale of ['zh', 'en'] as const) {
      const meta = getPermissionModeMeta(locale);
      const markup = render(locale, 'execute');

      assert.match(markup, new RegExp(escapeRegExp(escapeMarkup(meta.ask.label))));
      assert.doesNotMatch(markup, new RegExp(escapeRegExp(escapeMarkup(meta.execute.hint))));
    }
  });

  it('names the dangerous mode for what it grants, not for the mechanism it turns off (#1616)', () => {
    const zh = getPermissionModeMeta('zh');
    const en = getPermissionModeMeta('en');

    assert.equal(zh.bypass.label, '完全权限');
    assert.equal(en.bypass.label, 'Full access');
    assert.equal(zh.bypass.tone, 'destructive');
    assert.equal(en.bypass.tone, 'destructive');
    assert.match(render('zh', 'bypass'), /完全权限/);
    assert.match(render('en', 'bypass'), /Full access/);
  });

  it('never hands the reader the word "sandbox" in a permission label or hint (#1616)', () => {
    for (const locale of ['zh', 'en'] as const) {
      const copy = getConversationCopy(locale);
      for (const mode of PERMISSION_MODES) {
        const { label, hint } = copy.permissions.mode[mode];
        assert.doesNotMatch(`${label} ${hint}`, /沙箱|sandbox/i, `${locale} ${mode}`);
      }
      assert.doesNotMatch(copy.sandboxBoundary.title, /沙箱|sandbox/i, locale);
    }
  });
});

function escapeMarkup(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(
    /"/g,
    '&quot;',
  ).replace(/'/g, '&#x27;');
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
