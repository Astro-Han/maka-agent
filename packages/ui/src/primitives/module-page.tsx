// packages/ui/src/primitives/module-page.tsx
//
// The ONE shell every module page (计划提醒 / 每日回顾 / …) renders into.
//
// Built on Astryx `Layout` rather than a hand-rolled grid, following the
// `incident-console` page template — the vendor's archetype for exactly this
// page: dense rows, not cards, with a resizable inspector panel beside them.
// Layout owns what we used to own by hand: header/content/panel zones, the
// full-bleed divider under the header, padding collapse between adjacent
// slots, and scroll containment.
//
// Deliberately NO `contentWidth` clamp. A clamp centres each slot inside its
// own box, so the header centres across the full width while the content
// centres inside (width − panel) — the title stops lining up with the rows the
// moment the inspector opens. The vendor template is edge-to-edge and so is
// this; the detail pane already bounds the page.
//
// Header shape is the template's, too: one row — title, one quiet supporting
// line, then the actions. No lede paragraph, so the page opens straight into
// its content instead of spending three stacked tiers before the first row.

import type { ReactNode } from 'react';
import { HStack, Heading, StackItem, Text } from '@astryxdesign/core';
import { Layout, LayoutContent, LayoutHeader, LayoutPanel } from '@astryxdesign/core/Layout';
import { ResizeHandle, useResizable } from '@astryxdesign/core/Resizable';
import { useMediaQuery } from '@astryxdesign/core/hooks';
import { cn } from '../utils.js';

export interface ModulePageProps {
  /** Page title. Also the `main` landmark's accessible name. */
  title: string;
  /**
   * One quiet line beside the title — a live count, not a description
   * ("3 个进行中", "今天"). The vendor header carries a supporting line here
   * rather than a lede paragraph below.
   */
  meta?: ReactNode;
  /** Right-aligned header cluster: the primary action, then any overflow menu. */
  actions?: ReactNode;
  /** Page body. Rendered edge-to-edge; the caller owns its own padding. */
  children: ReactNode;
  /**
   * Inspector content for the end panel. `undefined` hides the panel and its
   * resize handle entirely — an empty panel is dead width, not a placeholder.
   */
  inspector?: ReactNode;
  /** Accessible name for the inspector panel landmark. */
  inspectorLabel?: string;
  /** localStorage key so a dragged inspector width survives a relaunch. */
  inspectorAutoSaveId?: string;
  className?: string;
}

export function ModulePage({
  title,
  meta,
  actions,
  children,
  inspector,
  inspectorLabel,
  inspectorAutoSaveId,
  className,
}: ModulePageProps) {
  // The `incident-console` template's own answer to a narrow window: drop the
  // end panel entirely rather than squeeze two columns into one. Below this
  // width the rows would have less room than their own content needs.
  const isNarrow = useMediaQuery('(max-width: 1024px)');
  // Hooks cannot be conditional, so the region always exists; only the panel
  // and its handle are conditional on there being something to inspect.
  const inspectorPanel = useResizable({
    defaultSize: 380,
    minSizePx: 300,
    maxSizePx: 560,
    autoSaveId: inspectorAutoSaveId,
  });

  return (
    <Layout
      height="fill"
      // The app shell is itself a full-bleed Layout, and
      // `--layout-padding-outer-x: 0` inherits into every nested one — the page
      // title would sit flush against the pane edge, 20px left of its own rows.
      // Restating the step here re-anchors the header on the content's column.
      padding={5}
      className={cn('maka-module-page', className)}
      header={
        <LayoutHeader hasDivider>
          <HStack gap={3} vAlign="center">
            <StackItem size="fill">
              <HStack gap={2} vAlign="center" wrap="wrap">
                <Heading level={1}>{title}</Heading>
                {meta != null ? (
                  <Text type="supporting" color="secondary">
                    {meta}
                  </Text>
                ) : null}
              </HStack>
            </StackItem>
            {actions}
          </HStack>
        </LayoutHeader>
      }
      content={<LayoutContent padding={0}>{children}</LayoutContent>}
      end={
        inspector != null && !isNarrow ? (
          <>
            <ResizeHandle direction="horizontal" isReversed resizable={inspectorPanel.props} hasDivider />
            <LayoutPanel resizable={inspectorPanel.props} label={inspectorLabel} padding={5}>
              {inspector}
            </LayoutPanel>
          </>
        ) : undefined
      }
    />
  );
}
