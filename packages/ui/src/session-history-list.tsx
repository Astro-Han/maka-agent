import { memo, useEffect, useRef, useState } from 'react';
import { useMountedRef } from './use-mounted-ref.js';
import type { ProjectRecord, SessionSummary, UiLocale } from '@maka/core';
import { formatCompactTimestamp } from '@maka/core';
import {
  AlertTriangle,
  Archive,
  ArchiveRestore,
  Ban,
  Bot,
  CircleCheckBig,
  Eye,
  FolderGit2,
  FolderOpen,
  Hourglass,
  Loader2,
  MessageSquare,
  MoreHorizontal,
  Pencil,
  Pin,
  PinOff,
  Plus,
  ShieldAlert,
  Trash2,
} from './icons.js';
import { EmptyState } from './empty-state.js';
import { OverlayScrollArea } from './overlay-scroll-area.js';
import {
  DropdownMenu,
  DropdownMenuItem,
} from '@astryxdesign/core/DropdownMenu';
import { Divider } from '@astryxdesign/core/Divider';
import { List, ListItem } from '@astryxdesign/core/List';
import { TreeList, type TreeListItemData } from '@astryxdesign/core/TreeList';
import { describeBlockedReason, presentSessionStatus } from './session-status-presentation.js';
import { useUiLocale } from './locale-context.js';
import { getConversationCopy } from './conversation-copy.js';

type SessionRowActionId = 'flag' | 'archive' | 'rename' | 'delete';
type SessionHistoryGroupVariant = 'conversation' | 'project';

export interface SessionRowActions {
  /** Flag (pin) state toggle. */
  onToggleFlag(sessionId: string, next: boolean): void | Promise<void>;
  /** Move to / out of the archive bucket. */
  onArchive(sessionId: string): void | Promise<void>;
  onUnarchive(sessionId: string): void | Promise<void>;
  /** Rename via inline prompt. Receives the new (trimmed) name. */
  onRename(sessionId: string, name: string): void | Promise<void>;
  /** Permanent removal — caller is responsible for the confirm gate. */
  onDelete(sessionId: string): void | Promise<void>;
}

export interface ProjectRowActions {
  onNew(projectId: string): void | Promise<void>;
  onRename(projectId: string, name: string): void | Promise<void>;
  onArchive(projectId: string): void | Promise<void>;
  onRestore(projectId: string): void | Promise<void>;
  onRelink(projectId: string): void | Promise<void>;
}

export interface SessionHistoryGroup {
  id: string;
  label: string;
  sessions: SessionSummary[];
  project?: ProjectRecord;
}

export function SessionHistoryList(props: {
  sessions: SessionSummary[];
  activeId?: string;
  /**
   * Per-session-id boolean flag: true when the session has a live streaming
   * delta in flight. Rendered as a small pulsing accent dot on the row.
   * Caller derives this from the live-turn projection so the sidebar
   * shows live activity without subscribing to the stream itself.
   */
  streamingSessionIds?: Set<string>;
  /**
   * Per-session-id boolean flag: true when the session's backend / connection
   * is stale (`backend='fake'` or `llmConnectionSlug` no longer resolves).
   * The row dims + shows a small "已过期" pill so users notice in the list
   * before clicking in and seeing the chat header banner. Caller derives this
   * by joining `sessions` against `connections` — keeps SessionListPanel
   * unaware of the connection store.
   */
  staleSessionIds?: Set<string>;
  /** Pre-computed groups used by the project view. */
  groups?: ReadonlyArray<SessionHistoryGroup>;
  worktreeSessionIds?: ReadonlySet<string>;
  projectActions?: ProjectRowActions;
  /** Linked subagent Sessions keyed by their durable parent Session id. */
  childSessionsByParentId?: ReadonlyMap<string, readonly SessionSummary[]>;
  groupVariant?: SessionHistoryGroupVariant;
  onSelectSession(sessionId: string): void;
  rowActions?: SessionRowActions;
}) {
  const locale = useUiLocale();
  const copy = getConversationCopy(locale).sessions;
  // 参考实现 keeps the lower sidebar region as stable chat history
  // even when Skills / Scheduled Tasks are open in the main pane.
  const sessionListTitle = copy.title;
  // PR-UX-POLISH-1 commit 4 (WAWQAQ msg `e0dbad11` + kenji msg
  // `2844f64f`): in-list `筛选会话` filter input removed. All search
  // capability lives in the top-level `搜索` modal (PR-SEARCH-MODAL-
  // REAL-0 wires it to the desktop preload's thread search in the same PR).
  // The previous `searchQuery` state + `searchInputRef` + ⌘F/Ctrl+F
  // focus binding are gone with it; ⌘F is freed for future use.
  // `filteredSessions` collapses to a direct passthrough of
  // `props.sessions` — group rendering downstream still partitions
  // by status / time / filter.

  return (
    <section className="maka-session-list" aria-label={sessionListTitle}>
      {props.sessions.length === 0 &&
      !(props.groupVariant === 'project' && (props.groups?.length ?? 0) > 0) ? (
        // WAWQAQ msg `f56f38c1` (2026-06-20): the create-session CTA
        // belongs in the sidebar header / nav rail, never in the
        // bottom session-history empty state. The empty state here is
        // pure "no sessions yet" copy — no inline CTA. The top-of-
        // sidebar `+ 新任务` button is the only create-session entry.
        <EmptyState
          Icon={MessageSquare}
          title={copy.emptyTitle}
          body={copy.emptyBody}
          extraClassName="maka-session-empty-state"
        />
      ) : (
        <OverlayScrollArea
          className="maka-list-stack"
          viewportClassName="maka-list-stackViewport"
          contentClassName="maka-list-stackContent"
        >
          <SessionListGroups
            groups={
              props.groups
                ? props.groups.map((g) => ({
                    key: g.id,
                    label: g.label,
                    sessions: g.sessions,
                    project: g.project,
                  }))
                : groupSessionsForHistory(props.sessions, locale).map((g) => ({
                    key: g.id,
                    label: g.label,
                    sessions: g.sessions,
                  }))
            }
            groupVariant={props.groupVariant ?? 'conversation'}
            activeId={props.activeId}
            streamingSessionIds={props.streamingSessionIds}
            staleSessionIds={props.staleSessionIds}
            childSessionsByParentId={props.childSessionsByParentId}
            worktreeSessionIds={props.worktreeSessionIds}
            onSelectSession={props.onSelectSession}
            rowActions={props.rowActions}
            projectActions={props.projectActions}
          />
        </OverlayScrollArea>
      )}
    </section>
  );
}

/** Render either the flat conversation groups or project disclosures. */
function SessionListGroups(props: {
  groups: ReadonlyArray<{
    key: string;
    label: string;
    sessions: SessionSummary[];
    project?: ProjectRecord;
  }>;
  groupVariant: SessionHistoryGroupVariant;
  activeId?: string;
  streamingSessionIds?: Set<string>;
  staleSessionIds?: Set<string>;
  childSessionsByParentId?: ReadonlyMap<string, readonly SessionSummary[]>;
  worktreeSessionIds?: ReadonlySet<string>;
  onSelectSession(sessionId: string): void;
  rowActions?: SessionRowActions;
  projectActions?: ProjectRowActions;
}) {
  if (props.groupVariant === 'project') {
    return (
      <ProjectHistoryTree
        {...props}
      />
    );
  }

  return (
    <>
      {props.groups.map((group) => {
        return (
          <List
            key={group.key}
            className="maka-session-history-group"
            density="compact"
            header={group.label ? (
              <div className="maka-list-group-label"><span>{group.label}</span></div>
            ) : undefined}
          >
            {group.sessions.map((session) => (
              <SessionListBranch
                key={session.id}
                session={session}
                activeId={props.activeId}
                streamingSessionIds={props.streamingSessionIds}
                staleSessionIds={props.staleSessionIds}
                childSessionsByParentId={props.childSessionsByParentId}
                worktreeSessionIds={props.worktreeSessionIds}
                onSelectSession={props.onSelectSession}
                rowActions={props.rowActions}
              />
            ))}
          </List>
        );
      })}
    </>
  );
}

interface HistoryTreeProps {
  groups: ReadonlyArray<{
    key: string;
    label: string;
    sessions: SessionSummary[];
    project?: ProjectRecord;
  }>;
  activeId?: string;
  streamingSessionIds?: Set<string>;
  staleSessionIds?: Set<string>;
  childSessionsByParentId?: ReadonlyMap<string, readonly SessionSummary[]>;
  worktreeSessionIds?: ReadonlySet<string>;
  onSelectSession(sessionId: string): void;
  rowActions?: SessionRowActions;
  projectActions?: ProjectRowActions;
}

function ProjectHistoryTree(props: HistoryTreeProps) {
  const locale = useUiLocale();
  const copy = getConversationCopy(locale).sessions;
  const [editingSessionId, setEditingSessionId] = useState<string>();
  const [editingProjectId, setEditingProjectId] = useState<string>();

  function sessionItem(session: SessionSummary, nested = false): TreeListItemData {
    const children = props.childSessionsByParentId?.get(session.id) ?? [];
    const editing = editingSessionId === session.id;
    return {
      id: `session:${session.id}`,
      label: editing ? (
        <InlineRenameInput
          value={session.name}
          ariaLabel={copy.renameAriaLabel}
          onCancel={() => setEditingSessionId(undefined)}
          onCommit={(name) => {
            setEditingSessionId(undefined);
            if (name && name !== session.name) void props.rowActions?.onRename(session.id, name);
          }}
        />
      ) : (
        <SessionItemLabel
          session={session}
          active={session.id === props.activeId}
          streaming={props.streamingSessionIds?.has(session.id)}
          stale={props.staleSessionIds?.has(session.id)}
          worktree={props.worktreeSessionIds?.has(session.id)}
          nested={nested}
        />
      ),
      description: editing ? formatSessionMeta(session, locale) : undefined,
      endContent: editing ? undefined : (
        <SessionItemEnd
          session={session}
          active={session.id === props.activeId}
          streaming={props.streamingSessionIds?.has(session.id)}
          actions={props.rowActions}
          onRename={() => setEditingSessionId(session.id)}
        />
      ),
      onClick: editing ? undefined : () => props.onSelectSession(session.id),
      isSelected: session.id === props.activeId,
      children: children.map((child) => sessionItem(child, true)),
    };
  }

  function projectItem(group: HistoryTreeProps['groups'][number]): TreeListItemData {
    const project = group.project;
    const editing = project != null && editingProjectId === project.id;
    return {
      id: project ? `project:${project.id}` : group.key,
      label: editing && project ? (
        <InlineRenameInput
          value={project.name}
          ariaLabel={copy.projectRename}
          onCancel={() => setEditingProjectId(undefined)}
          onCommit={(name) => {
            setEditingProjectId(undefined);
            if (name && name !== project.name) void props.projectActions?.onRename(project.id, name);
          }}
        />
      ) : (
        <span className="maka-project-tree-label">
          <span>{group.label}</span>
          <span className="maka-project-tree-count">{group.sessions.length}</span>
        </span>
      ),
      startContent: <FolderOpen size={14} aria-hidden="true" />,
      endContent: project && !editing ? (
        <span className="maka-project-tree-actions">
          {!project.available && <AlertTriangle size={12} aria-label={copy.projectUnavailable} />}
          {props.projectActions && (
            <ProjectActionsMenu
              project={project}
              actions={props.projectActions}
              onRename={() => setEditingProjectId(project.id)}
            />
          )}
        </span>
      ) : undefined,
      isExpanded: group.sessions.length > 0,
      children: group.sessions.map((session) => sessionItem(session)),
    };
  }

  const active = props.groups.filter((group) => group.project?.archivedAt === undefined);
  const archived = props.groups.filter((group) => group.project?.archivedAt !== undefined);
  const items: TreeListItemData[] = active.map(projectItem);
  if (archived.length > 0) {
    items.push({
      id: 'archived-projects',
      label: copy.archivedProjects,
      startContent: <Archive size={14} aria-hidden="true" />,
      endContent: <span className="maka-project-tree-count">{archived.length}</span>,
      children: archived.map(projectItem),
    });
  }

  return (
    <TreeList
      className="maka-project-history-tree"
      density="balanced"
      variant="lineGuides"
      items={items}
      onKeyDown={(event) => {
        if (event.key !== 'Delete' && event.key !== 'Backspace') return;
        if ((event.target as HTMLElement).closest('input, [data-session-actions]')) return;
        const item = (event.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]');
        const sessionId = item?.dataset.treeId?.startsWith('session:')
          ? item.dataset.treeId.slice('session:'.length)
          : undefined;
        if (!sessionId || !props.rowActions) return;
        event.preventDefault();
        void props.rowActions.onDelete(sessionId);
      }}
    />
  );
}

function InlineRenameInput(props: {
  value: string;
  ariaLabel: string;
  onCommit(value: string): void;
  onCancel(): void;
}) {
  const ref = useRef<HTMLInputElement>(null);
  const cancelledRef = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  return (
    <input
      ref={ref}
      className="maka-session-inline-rename"
      defaultValue={props.value}
      maxLength={80}
      aria-label={props.ariaLabel}
      autoComplete="off"
      spellCheck={false}
      onBlur={(event) => {
        if (cancelledRef.current) return;
        props.onCommit(event.currentTarget.value.trim());
      }}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing || event.key === 'Process') return;
        if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
          event.stopPropagation();
          return;
        }
        if (event.key === 'Enter') {
          event.preventDefault();
          event.stopPropagation();
          props.onCommit(event.currentTarget.value.trim());
        } else if (event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          cancelledRef.current = true;
          props.onCancel();
        }
      }}
    />
  );
}

/**
 * Small inline icon next to the session name representing its
 * lifecycle status. Hidden for `active`
 * since that's the default and would add visual noise to most rows.
 *
 * `aborted` is rendered as muted history: not an error, not active,
 * and not silently swallowed.
 *
 * Caller is expected to pass a session with a SessionStatus from
 * `@maka/core` — typed as the SessionSummary from props avoids
 * pulling the core type into this file's import list.
 */
function SessionStatusIcon(props: { session: SessionSummary }) {
  const locale = useUiLocale();
  const { session } = props;
  const status = session.status;
  // Active is the default; no icon to reduce noise. Aborted retains a
  // muted icon (per @kenji review on PR109b — aborted is dormant
  // history that must remain visible, not silently swallowed as active).
  if (status === 'active') return null;
  const Icon = STATUS_ICON_BY_STATUS[status as keyof typeof STATUS_ICON_BY_STATUS];
  if (!Icon) return null;
  const { label, tone } = presentSessionStatus(status, locale);
  // `blocked` may attach a reason; we surface the generalized text in
  // the tooltip without exposing the raw enum identifier (per @kenji
  // i18n contract). The shared presentation module owns the mapping so
  // sidebar and renderer surfaces cannot drift.
  const blockedDetail = status === 'blocked' && session.blockedReason
    ? describeBlockedReason(session.blockedReason, locale)
    : null;
  const title = blockedDetail ? `${label} · ${blockedDetail}` : label;
  return (
    <span
      className="maka-list-row-status-icon"
      data-tone={tone}
      data-status={status}
      aria-label={title}
      title={title}
    >
      <Icon size={12} aria-hidden="true" />
    </span>
  );
}

/**
 * PawWork-style sidebar attention priority: asking/busy/error outrank unread,
 * and unread outranks plain time. The status icon beside the name already
 * carries asking/busy/error in Maka, so the right slot only shows the unread
 * dot when no higher-priority row state is active.
 */
function shouldShowSessionUnreadDot(session: SessionSummary, streaming: boolean, active: boolean): boolean {
  if (active) return false;
  if (!session.hasUnread) return false;
  if (streaming) return false;
  return !SIDEBAR_UNREAD_SUPPRESSED_STATUSES.has(session.status);
}

const SIDEBAR_UNREAD_SUPPRESSED_STATUSES = new Set<string>([
  'running',
  'waiting_for_user',
  'blocked',
]);

const STATUS_ICON_BY_STATUS = {
  running: Loader2,
  waiting_for_user: Hourglass,
  blocked: ShieldAlert,
  review: Eye,
  done: CircleCheckBig,
  archived: Archive,
  aborted: Ban,
} as const;

function SessionListBranch(props: {
  session: SessionSummary;
  activeId?: string;
  streamingSessionIds?: Set<string>;
  staleSessionIds?: Set<string>;
  childSessionsByParentId?: ReadonlyMap<string, readonly SessionSummary[]>;
  worktreeSessionIds?: ReadonlySet<string>;
  onSelectSession(sessionId: string): void;
  rowActions?: SessionRowActions;
  depth?: number;
}) {
  const depth = props.depth ?? 0;
  const children = props.childSessionsByParentId?.get(props.session.id) ?? [];
  return (
    <>
      <SessionListItem
        session={props.session}
        active={props.session.id === props.activeId}
        streaming={props.streamingSessionIds?.has(props.session.id)}
        stale={props.staleSessionIds?.has(props.session.id)}
        worktree={props.worktreeSessionIds?.has(props.session.id)}
        nested={depth > 0}
        onSelect={props.onSelectSession}
        actions={props.rowActions}
      />
      {children.map((child) => (
        <SessionListBranch key={child.id} {...props} session={child} depth={depth + 1} />
      ))}
    </>
  );
}

const SessionListItem = memo(function SessionListItem(props: {
  session: SessionSummary;
  active: boolean;
  streaming?: boolean;
  stale?: boolean;
  worktree?: boolean;
  nested?: boolean;
  onSelect(sessionId: string): void;
  actions?: SessionRowActions;
}) {
  const { session, active, streaming, stale, worktree, nested, actions } = props;
  const locale = useUiLocale();
  const copy = getConversationCopy(locale).sessions;
  const [editing, setEditing] = useState(false);
  return (
    <ListItem
      className="maka-session-list-item"
      data-session-id={session.id}
      data-subagent={nested ? 'true' : undefined}
      data-stale={stale ? 'true' : undefined}
      isSelected={active}
      label={editing ? (
        <InlineRenameInput
          value={session.name}
          ariaLabel={copy.renameAriaLabel}
          onCancel={() => setEditing(false)}
          onCommit={(name) => {
            setEditing(false);
            if (name && name !== session.name) void actions?.onRename(session.id, name);
          }}
        />
      ) : (
        <SessionItemLabel
          session={session}
          active={active}
          streaming={streaming}
          stale={stale}
          worktree={worktree}
          nested={nested}
        />
      )}
      description={editing ? formatSessionMeta(session, locale) : undefined}
      endContent={editing ? undefined : (
        <SessionItemEnd
          session={session}
          active={active}
          streaming={streaming}
          actions={actions}
          onRename={() => setEditing(true)}
        />
      )}
      onClick={editing ? undefined : () => props.onSelect(session.id)}
      onKeyDown={(event) => {
        if (event.key !== 'Delete' && event.key !== 'Backspace') return;
        if ((event.target as HTMLElement).closest('input, [data-session-actions]')) return;
        if (!actions) return;
        event.preventDefault();
        void actions.onDelete(session.id);
      }}
    />
  );
});

function SessionItemLabel(props: {
  session: SessionSummary;
  active: boolean;
  streaming?: boolean;
  stale?: boolean;
  worktree?: boolean;
  nested?: boolean;
}) {
  const copy = getConversationCopy(useUiLocale()).sessions;
  return (
    <span
      className="maka-session-item-label"
      data-active={props.active ? 'true' : undefined}
      data-stale={props.stale ? 'true' : undefined}
    >
      {props.nested && <Bot size={12} aria-hidden="true" />}
      {props.worktree && <FolderGit2 size={12} aria-label={copy.worktreeAriaLabel} />}
      {props.streaming && (
        <span className="maka-list-row-streaming-dot" aria-label={copy.respondingAriaLabel} title={copy.respondingTitle} />
      )}
      <SessionStatusIcon session={props.session} />
      <span>{props.session.name}</span>
      {props.stale && (
        <span className="maka-list-row-stale-pill" title={copy.staleTitle} aria-label={copy.staleAriaLabel}>
          {copy.stale}
        </span>
      )}
    </span>
  );
}

function SessionItemEnd(props: {
  session: SessionSummary;
  active: boolean;
  streaming?: boolean;
  actions?: SessionRowActions;
  onRename(): void;
}) {
  const locale = useUiLocale();
  const copy = getConversationCopy(locale).sessions;
  return (
    <span className="maka-session-item-end">
      {shouldShowSessionUnreadDot(props.session, Boolean(props.streaming), props.active) ? (
        <span className="maka-list-row-unread" aria-label={copy.unreadAriaLabel} />
      ) : (
        <span className="maka-session-item-meta">{formatSessionMeta(props.session, locale)}</span>
      )}
      {props.actions && (
        <SessionActionsMenu session={props.session} actions={props.actions} onRename={props.onRename} />
      )}
    </span>
  );
}

function SessionActionsMenu(props: {
  session: SessionSummary;
  actions: SessionRowActions;
  onRename(): void;
}) {
  const copy = getConversationCopy(useUiLocale()).sessions;
  const [menuOpen, setMenuOpen] = useState(false);
  const [pendingAction, setPendingAction] = useState<SessionRowActionId>();
  const mountedRef = useMountedRef();
  const pendingRef = useRef<SessionRowActionId | undefined>(undefined);
  const deleteAfterCloseRef = useRef(false);
  useEffect(() => () => { pendingRef.current = undefined; }, []);

  function run(actionId: SessionRowActionId, action: () => void | Promise<void>) {
    if (pendingRef.current) return;
    pendingRef.current = actionId;
    setPendingAction(actionId);
    void Promise.resolve(action()).catch(() => {}).finally(() => {
      pendingRef.current = undefined;
      if (mountedRef.current) setPendingAction(undefined);
    });
  }

  return (
    <span data-session-actions="" className="maka-session-item-actions">
      <DropdownMenu
        isMenuOpen={menuOpen}
        onOpenChange={(open) => {
          setMenuOpen(open);
          if (!open && deleteAfterCloseRef.current) {
            deleteAfterCloseRef.current = false;
            window.requestAnimationFrame(() => run('delete', () => props.actions.onDelete(props.session.id)));
          }
        }}
        button={{
          label: copy.actionsAriaLabel,
          icon: pendingAction ? <Loader2 size={14} aria-hidden="true" /> : <MoreHorizontal size={16} aria-hidden="true" />,
          isIconOnly: true,
          variant: 'ghost',
          size: 'sm',
          className: 'maka-session-item-menu-trigger',
        }}
      >
        <DropdownMenuItem
          isDisabled={pendingAction !== undefined}
          onClick={() => run('flag', () => props.actions.onToggleFlag(props.session.id, !props.session.isFlagged))}
          icon={props.session.isFlagged ? <PinOff size={16} aria-hidden="true" /> : <Pin size={16} aria-hidden="true" />}
          label={props.session.isFlagged ? copy.unpin : copy.pin}
        />
        <DropdownMenuItem isDisabled={pendingAction !== undefined} onClick={props.onRename} icon={<Pencil size={16} aria-hidden="true" />} label={copy.rename} />
        <DropdownMenuItem
          isDisabled={pendingAction !== undefined}
          onClick={() => run('archive', () => props.session.isArchived ? props.actions.onUnarchive(props.session.id) : props.actions.onArchive(props.session.id))}
          icon={props.session.isArchived ? <ArchiveRestore size={16} aria-hidden="true" /> : <Archive size={16} aria-hidden="true" />}
          label={props.session.isArchived ? copy.unarchive : copy.archive}
        />
        <Divider orientation="horizontal" />
        <DropdownMenuItem
          isDisabled={pendingAction !== undefined}
          onClick={() => { deleteAfterCloseRef.current = true; }}
          icon={<Trash2 size={16} aria-hidden="true" />}
          label={copy.delete}
          style={{ color: 'var(--destructive-text)' }}
        />
      </DropdownMenu>
    </span>
  );
}

function ProjectActionsMenu(props: {
  project: ProjectRecord;
  actions: ProjectRowActions;
  onRename(): void;
}) {
  const copy = getConversationCopy(useUiLocale()).sessions;
  const [pending, setPending] = useState<string>();
  const mountedRef = useMountedRef();
  const pendingRef = useRef<string | undefined>(undefined);
  useEffect(() => () => { pendingRef.current = undefined; }, []);
  function run(id: string, action: () => void | Promise<void>) {
    if (pendingRef.current) return;
    pendingRef.current = id;
    setPending(id);
    void Promise.resolve(action()).catch(() => {}).finally(() => {
      pendingRef.current = undefined;
      if (mountedRef.current) setPending(undefined);
    });
  }
  return (
    <DropdownMenu button={{
      label: copy.projectActionsAriaLabel(props.project.name),
      icon: pending ? <Loader2 size={14} aria-hidden="true" /> : <MoreHorizontal size={14} aria-hidden="true" />,
      isIconOnly: true,
      variant: 'ghost',
      size: 'sm',
      className: 'maka-project-tree-menu-trigger',
      isDisabled: pending !== undefined,
    }}>
      {props.project.archivedAt !== undefined ? (
        <DropdownMenuItem onClick={() => run('restore', () => props.actions.onRestore(props.project.id))} icon={<ArchiveRestore size={15} aria-hidden="true" />} label={copy.projectRestore} />
      ) : (
        <>
          {props.project.available ? (
            <DropdownMenuItem onClick={() => run('new', () => props.actions.onNew(props.project.id))} icon={<Plus size={15} aria-hidden="true" />} label={copy.projectNewTask} />
          ) : (
            <DropdownMenuItem onClick={() => run('relink', () => props.actions.onRelink(props.project.id))} icon={<FolderOpen size={15} aria-hidden="true" />} label={copy.projectRelink} />
          )}
          <Divider orientation="horizontal" />
          <DropdownMenuItem onClick={props.onRename} icon={<Pencil size={15} aria-hidden="true" />} label={copy.projectRename} />
          <DropdownMenuItem onClick={() => run('archive', () => props.actions.onArchive(props.project.id))} icon={<Archive size={15} aria-hidden="true" />} label={copy.projectArchive} />
        </>
      )}
    </DropdownMenu>
  );
}

interface SessionGroup {
  id: 'pinned' | 'unpinned';
  label: string;
  sessions: SessionSummary[];
}

function groupSessionsForHistory(sessions: SessionSummary[], locale: UiLocale): SessionGroup[] {
  const copy = getConversationCopy(locale).sessions;
  const ordered = [...sessions].sort((a, b) => {
    const timestampDelta = (b.lastMessageAt ?? 0) - (a.lastMessageAt ?? 0);
    return timestampDelta || a.id.localeCompare(b.id);
  });
  const pinned = ordered.filter((session) => session.isFlagged);
  const unpinned = ordered.filter((session) => !session.isFlagged);
  const groups: SessionGroup[] = [];
  if (pinned.length > 0) {
    groups.push({ id: 'pinned', label: copy.pinned, sessions: pinned });
  }
  if (unpinned.length > 0) {
    groups.push({ id: 'unpinned', label: '', sessions: unpinned });
  }
  return groups;
}

function formatSessionMeta(session: SessionSummary, locale: UiLocale): string {
  if (!session.lastMessageAt) return getConversationCopy(locale).chat.noMessages;
  return formatCompactTimestamp(session.lastMessageAt, Date.now(), locale);
}
