import type { PlanReminder } from '@maka/core';
import type { CSSProperties } from 'react';
import { Blocks, Settings, SquarePen, Timer } from './icons.js';
import type { NavModuleMemory, NavSelection } from './nav-selection.js';
import { Button as BaseButton } from '@base-ui/react/button';
import { SideNavItem } from '@astryxdesign/core/SideNav';
import { useUiLocale } from './locale-context.js';
import { getShellControlsCopy } from './shell-controls-copy.js';

export function SessionSidebarNav(props: {
  selection: NavSelection;
  planReminders?: PlanReminder[];
  moduleMemory?: NavModuleMemory;
  onSelect(selection: NavSelection): void;
  onNew(): void;
}) {
  const locale = useUiLocale();
  const copy = getShellControlsCopy(locale).navigation;
  const extensionsActive = props.selection.section === 'extensions';
  const automationsActive = props.selection.section === 'automations';
  const moduleMemory = props.moduleMemory ?? { extensions: 'skills', automations: 'plan-reminders' };
  const activePlanReminderCount = (props.planReminders ?? []).filter(
    (reminder) => reminder.status !== 'completed',
  ).length;

  return (
    <div className="maka-sidebar-modules" role="navigation" aria-label={copy.mainLabel}>
      <SideNavItem
        label={copy.newTask}
        icon={<SquarePen aria-hidden="true" />}
        endContent={<kbd className="maka-nav-kbd" aria-hidden="true">⌘ N</kbd>}
        size="sm"
        onClick={props.onNew}
      />
      <SideNavItem
        label={copy.extensions}
        icon={<Blocks aria-hidden="true" />}
        isSelected={extensionsActive}
        size="sm"
        onClick={() => props.onSelect({ section: 'extensions', module: moduleMemory.extensions })}
      />
      <SideNavItem
        label={copy.automations}
        icon={<Timer aria-hidden="true" />}
        isSelected={automationsActive}
        size="sm"
        onClick={() => props.onSelect({ section: 'automations', module: moduleMemory.automations })}
        endContent={activePlanReminderCount > 0 ? (
          <>
            <small className="maka-nav-count" aria-hidden="true">{activePlanReminderCount}</small>
            <span className="maka-nav-pending-copy">
              {copy.pendingReminders(activePlanReminderCount).slice(copy.automations.length)}
            </span>
          </>
        ) : undefined}
      />
    </div>
  );
}

export type SidebarUpdateReminder = {
  state: 'available' | 'downloading' | 'downloaded' | 'error';
  latestVersion: string;
  progressPercent?: number;
};

export function SessionSidebarFooter(props: {
  updateReminder?: SidebarUpdateReminder;
  onOpenSettings(): void;
  onOpenUpdate?(): void;
}) {
  const locale = useUiLocale();
  const copy = getShellControlsCopy(locale).navigation;
  const updatePercent = Math.round(props.updateReminder?.progressPercent ?? 0);
  const updateLabel = props.updateReminder?.state === 'downloaded'
    ? copy.restartUpdate
    : props.updateReminder?.state === 'downloading'
      ? `${updatePercent}%`
      : copy.update;
  const updateTitle = props.updateReminder?.state === 'downloaded'
    ? copy.updateDownloaded(props.updateReminder.latestVersion)
    : props.updateReminder?.state === 'downloading'
      ? copy.downloadingUpdate(updatePercent)
      : props.updateReminder
        ? copy.updateAvailable(props.updateReminder.latestVersion)
        : copy.update;
  return (
    <footer className="maka-session-panel-footer">
      <SideNavItem
        label={copy.settings}
        icon={<Settings aria-hidden="true" />}
        size="sm"
        onClick={props.onOpenSettings}
      />
      {props.updateReminder && props.onOpenUpdate && (
        <BaseButton
          className="maka-sidebar-update-button"
          data-update-state={props.updateReminder.state}
          style={{ '--maka-update-progress': String(Math.max(0, Math.min(100, props.updateReminder.progressPercent ?? 0)) / 100) } as CSSProperties}
          type="button"
          onClick={props.onOpenUpdate}
          disabled={props.updateReminder.state === 'downloading'}
          aria-label={updateTitle}
          title={updateTitle}
        >
          {props.updateReminder.state === 'downloading' && <span className="maka-sidebar-update-progress" aria-hidden="true" />}
          <span>{updateLabel}</span>
        </BaseButton>
      )}
    </footer>
  );
}
