// apps/desktop/src/renderer/settings/settings-section.tsx
//
// The ONE grouping unit for a settings page.
//
// Before this, the 14 settings pages shared no page vocabulary. 通用 stacked
// four Astryx `Card`s with NO titles — four unlabeled boxes whose grouping the
// user had to infer. 外观 used no cards at all. 权限与能力 opened with a
// `SectionHeader` repeating the page title verbatim. Five different page-root
// containers existed (`settingsStructuredPage`, `settingsUsagePage`,
// `settingsFeatureStatusPage`, `settingsHealthPage`, `settingsAboutPage`) and
// 222 bespoke `.settings*` selectors carried the difference.
//
// A settings page is a list of LABELED GROUPS. That is the whole model:
// a group states what it configures, optionally why, optionally offers one
// group-level action, and then lists its rows. `SettingsSection` is that unit,
// so a page becomes a flat list of sections and stops inventing layout.
//
// `variant`:
//   'rows' (default) — the body is the shared `.settingsRows` card, the single
//     card primitive on the settings surface. Rows are Astryx `Item`s.
//   'bare' — the body is a plain block, for groups whose content is not a row
//     list (the 外观 option grids, a form layout, a chart). The section still
//     contributes its header and its share of the page rhythm, which is the
//     point: 'bare' opts out of the CARD, never out of the vocabulary.
import type { ReactNode } from 'react';
import { Card, Heading, HStack, Item, Text, VStack } from '@astryxdesign/core';
import { cn } from '@maka/ui';

export function SettingsSection(props: {
  /** Group label. Omit only for a page's single unlabeled lead group. */
  title?: ReactNode;
  /** One quiet line under the title explaining what the group governs. */
  description?: ReactNode;
  /** Group-level action cluster (refresh, add, filter), right-aligned. */
  action?: ReactNode;
  variant?: 'rows' | 'bare';
  className?: string;
  /** Class for the body element, when a page needs to pin its own grid. */
  bodyClassName?: string;
  children: ReactNode;
}) {
  const hasHeader = props.title != null || props.description != null || props.action != null;
  return (
    <section className={cn('settingsSection', props.className)}>
      {hasHeader ? (
        /* The header is Astryx's own settings idiom — `Heading level={3}` over
           a `Text type="supporting" color="secondary"` lede — as used by the
           `settings` and `settings-sidebar` page templates the CLI vendors.
           It was @maka/ui's SectionHeader, which styles the same two lines with
           hand-written Tailwind (`text-[length:var(--font-size-ui)]
           font-semibold`, a caption-sized subtitle). Deferring to Astryx means
           section typography now moves with the theme instead of with a copy
           of the theme's values. */
        <HStack gap={3} align="start" justify="between">
          <VStack gap={0.5}>
            {props.title != null ? <Heading level={3}>{props.title}</Heading> : null}
            {props.description != null ? (
              <Text type="supporting" size="sm" color="secondary">{props.description}</Text>
            ) : null}
          </VStack>
          {props.action != null ? <div>{props.action}</div> : null}
        </HStack>
      ) : null}
      {props.variant === 'bare' ? (
        <div className={cn('settingsSectionBody', props.bodyClassName)}>{props.children}</div>
      ) : (
        <Card padding={0} className={cn('settingsRows', props.bodyClassName)}>
          {props.children}
        </Card>
      )}
    </section>
  );
}

/**
 * The ONE row unit inside a 'rows' section: label + wrapping helper line on
 * the left, one control (or read-only value) on the right. Astryx `Item` is
 * the layout; this wrapper exists for two Astryx behaviors that are wrong
 * for a settings surface, fixed once here instead of per call site:
 *
 * 1. `Item` single-line-truncates STRING descriptions. A settings helper
 *    line ("switching applies immediately and persists…") must wrap, never
 *    ellipsize — the truncated tail is exactly the part that says what the
 *    control does. Wrapping the text in a fragment makes it a ReactNode,
 *    which `Item` renders without truncation; the description span's
 *    inherited type styles still apply.
 * 2. The end slot needs a bounded share of the row. An unbounded control
 *    (SegmentedControl with English labels, a model picker trigger) would
 *    otherwise crush the label column to nothing before it wraps —
 *    `.settingsRowEnd` caps it and lets the container query in rows.css
 *    stack it under the label on narrow cards.
 *
 * `density="spacious"` (12px block+inline) is the library's own inset that
 * matches `.settingsFieldRow` padding, so Item rows and form rows in the
 * same card share one left edge.
 */
export function SettingsRow(props: {
  label: ReactNode;
  description?: ReactNode;
  /** The row's control / value cluster, right-aligned. */
  end?: ReactNode;
  align?: 'center' | 'start';
  children?: never;
}) {
  return (
    <Item
      density="spacious"
      align={props.align}
      label={props.label}
      description={props.description == null ? undefined : <>{props.description}</>}
      endContent={props.end == null ? undefined : <span className="settingsRowEnd">{props.end}</span>}
    />
  );
}

/**
 * A full-width form block inside a 'rows' section — a `FormLayout`, one wide
 * `TextInput`/`TextArea`, or a preview body. Owns the same 12px inset as
 * `SettingsRow` via padding (not margin), so the card's hairline dividers
 * span the full card width on either side of it.
 */
export function SettingsField(props: { className?: string; children: ReactNode }) {
  return <div className={cn('settingsFieldRow', props.className)}>{props.children}</div>;
}

/** A trailing action cluster row (test/export/import buttons) in a 'rows' card. */
export function SettingsActions(props: { role?: string; 'aria-label'?: string; children: ReactNode }) {
  return (
    <div className="settingsFieldRow settingsActionRow" role={props.role} aria-label={props['aria-label']}>
      {props.children}
    </div>
  );
}
