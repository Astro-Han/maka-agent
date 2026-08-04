// packages/ui/src/primitives/module-page.tsx
//
// The ONE shell every module page (计划提醒 / 每日回顾 / …) renders into.
//
// Built on Astryx `Layout` rather than a hand-rolled grid, following the
// `incident-console` page template — the vendor's archetype for exactly this
// page: dense rows, not cards, with a resizable inspector panel beside them.
// Layout owns what we used to own by hand: header/content/panel zones,
// padding collapse between adjacent slots, and scroll containment.
//
// `contentWidth` clamps the page into a centred column, the way every other
// main page in this app is clamped — nothing here runs edge to edge. Astryx
// applies the same `--layout-content-width` to the header's inner wrapper and
// to the body row, and both centre inside the full plate width, so the title
// keeps the rows' left edge. The inspector rides INSIDE that column rather
// than beside it, which is what keeps the two aligned when it opens.
//
// Header shape is the template's, too: title, one quiet supporting line and
// the actions on one row, then the page's control bar as the header's last
// row. No lede paragraph, so the page opens straight into its content instead
// of spending three stacked tiers before the first row.
//
// No rules anywhere in the page chrome — not under the header, not beside the
// inspector. The only lines on the page are the list's own row dividers, which
// say where a row ends; a header rule and a panel rule would only restate a
// boundary the layout already draws, and this app separates columns tonally
// (DESIGN.md, One Working Plane) rather than with hairlines.

import type { ReactNode } from 'react';
import { HStack, Heading, StackItem, Text, VStack } from '@astryxdesign/core';
import { Layout, LayoutContent, LayoutHeader, LayoutPanel } from '@astryxdesign/core/Layout';
import { ResizeHandle, useResizable } from '@astryxdesign/core/Resizable';
import { useMediaQuery } from '@astryxdesign/core/hooks';
import { cn } from '../utils.js';

/**
 * The page column's max width. Matches the clamp the skills / MCP / settings
 * pages already use, so every main page shares one measure.
 */
const MODULE_PAGE_WIDTH = 900;

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
  /**
   * The page's control bar — module switch on the left, this page's view and
   * filters on the right. It belongs to the whole page, not to the list, so it
   * lives in the header above the content/inspector split. Its own bottom
   * hairline is then the ONE horizontal rule on the page, and the inspector's
   * vertical divider starts exactly where that rule ends.
   */
  toolbar?: ReactNode;
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
  toolbar,
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
  // Sized against the clamped column, not the window: the inspector shares
  // MODULE_PAGE_WIDTH with the rows, so a panel tuned for a full-bleed page
  // would leave the list too little of it.
  const inspectorPanel = useResizable({
    defaultSize: 320,
    minSizePx: 280,
    maxSizePx: 420,
    autoSaveId: inspectorAutoSaveId,
  });

  return (
    <Layout
      height="fill"
      contentWidth={MODULE_PAGE_WIDTH}
      // The app shell is itself a full-bleed Layout, and
      // `--layout-padding-outer-x: 0` inherits into every nested one — the page
      // title would sit flush against the pane edge, 20px left of its own rows.
      // Restating the step here re-anchors the header on the content's column.
      padding={5}
      className={cn('maka-module-page', className)}
      header={
        <LayoutHeader>
          <VStack gap={4}>
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
            {toolbar}
          </VStack>
        </LayoutHeader>
      }
      content={<LayoutContent padding={0}>{children}</LayoutContent>}
      end={
        inspector != null && !isNarrow ? (
          <>
            {/* No divider: the panel's own edge is already the boundary, and
                a standing rule here would be a second one drawn on top of it.
                The handle stays a hover-revealed grip, which is the vendor's
                own no-divider mode. */}
            <ResizeHandle direction="horizontal" isReversed resizable={inspectorPanel.props} />
            <LayoutPanel resizable={inspectorPanel.props} label={inspectorLabel} padding={5}>
              {inspector}
            </LayoutPanel>
          </>
        ) : undefined
      }
    />
  );
}
