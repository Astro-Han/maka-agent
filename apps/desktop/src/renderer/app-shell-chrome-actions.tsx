import {
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Search,
} from '@maka/ui/icons';
import {
  IconButton,
  useUiLocale,
} from '@maka/ui';
import { Icon } from '@astryxdesign/core/Icon';
import { Tooltip } from '@astryxdesign/core/Tooltip';
import {
  SideNavCollapseButton,
  type SideNavImperativeCollapseHandle,
} from '@astryxdesign/core/SideNav';
import type { RefObject } from 'react';
import { getShellCopy } from './locales/shell-copy';

/**
 * Match SideNavItem collapsed/expanded icon slot: Astryx `renderIconSlot`
 * uses size `sm` (1rem) + color `secondary`. Titlebar follows that recipe
 * — not raw Lucide size props — so chrome and sidebar share one glyph look.
 */
function ChromeIcon(props: { icon: typeof Search }) {
  return <Icon icon={props.icon} size="sm" color="secondary" />;
}

export function AppShellTopbarActions(props: {
  sidebarCollapsed: boolean;
  sidebarHandleRef: RefObject<SideNavImperativeCollapseHandle | null>;
  /* Settings has its own navigation column; the session-sidebar toggle is
     meaningless there. */
  sidebarToggleHidden?: boolean;
  onOpenSearchModal(): void;
}) {
  const locale = useUiLocale();
  const copy = getShellCopy(locale).chrome;
  return (
    <div className="maka-shell-topbar-rail" data-maka-contract="shell-topbar-rail" role="group" aria-label={copy.windowActions}>
      <Tooltip content={copy.searchConversations}>
        <IconButton
          label={copy.searchConversations}
          icon={<ChromeIcon icon={Search} />}
          variant="ghost"
          size="md"
          className="maka-titlebar-action"
          data-maka-search-trigger="true"
          onClick={props.onOpenSearchModal}
        />
      </Tooltip>
      {!props.sidebarToggleHidden && (
      <Tooltip content={props.sidebarCollapsed ? copy.expandSidebar : copy.collapseSidebar}>
        <SideNavCollapseButton
          handleRef={props.sidebarHandleRef}
          label={props.sidebarCollapsed ? copy.expandSidebar : copy.collapseSidebar}
          className="maka-titlebar-action"
          aria-expanded={!props.sidebarCollapsed}
        >
          {props.sidebarCollapsed ? (
            <ChromeIcon icon={PanelLeftOpen} />
          ) : (
            <ChromeIcon icon={PanelLeftClose} />
          )}
        </SideNavCollapseButton>
      </Tooltip>
      )}
      {/* Collapsed "new task" lives on the SideNav rail (SessionSidebarNav),
          not here — a third titlebar button duplicated the rail icon and made
          left-cluster width state-dependent for drag-region math. */}
    </div>
  );
}

/**
 * The titlebar's right edge.
 *
 * It used to also carry a `…` menu holding 问题反馈 / 打开命令面板 / 打开帮助 /
 * 打开健康中心 — a drawer of four things that each belong somewhere else.
 * 健康中心 was a duplicate of the Settings nav entry, 问题反馈 only opened
 * Settings → 关于, and the other two now have real homes: the keyboard sheet is
 * a row on 关于, and the palette keeps ⌘K, which that sheet documents.
 *
 * What is left is the workbar toggle — the right-hand mirror of the sidebar
 * toggle above, and positioned the same way: one control that stays put across
 * its own state change. It used to hand itself off to a second button inside
 * the workbar's tab row while the workbar was open, so the single control a
 * user clicks twice moved ~30px down and left between those two clicks.
 *
 * The titlebar is also the only row in the app that already knows where the
 * platform's native window controls are: `.maka-window-titlebar` reserves
 * `env(titlebar-area-*)` on both sides, so this button clears the macOS traffic
 * lights and the Windows caption strip without either platform being named
 * here. A toggle parked anywhere else would have to restate that.
 */
export function AppShellWorkspaceTopActions(props: {
  workbarAvailable: boolean;
  workbarCollapsed: boolean;
  onToggleWorkbar(): void;
}) {
  const locale = useUiLocale();
  const copy = getShellCopy(locale).chrome;
  // Nothing to toggle outside a session; an empty no-drag rectangle in the
  // titlebar would only subtract from the window's drag surface.
  if (!props.workbarAvailable) return null;
  const label = props.workbarCollapsed ? copy.expandWorkbar : copy.collapseWorkbar;

  return (
    <div className="maka-workspace-top-actions" role="toolbar" aria-label={copy.workspaceActions}>
      <Tooltip content={label}>
        <IconButton
          label={label}
          icon={<ChromeIcon icon={props.workbarCollapsed ? PanelRightOpen : PanelRightClose} />}
          variant="ghost"
          size="md"
          className="maka-titlebar-action"
          onClick={props.onToggleWorkbar}
          aria-expanded={!props.workbarCollapsed}
        />
      </Tooltip>
    </div>
  );
}
