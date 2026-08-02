/** Either a React synthetic event or the native one it wraps. */
export interface ChatInputCompositionEvent {
  key?: string;
  nativeEvent?: object;
}

export function isChatInputComposing(
  event: ChatInputCompositionEvent,
  trackedComposition = false,
): boolean {
  const native: object = event.nativeEvent ?? event;
  return trackedComposition || event.key === 'Process'
    || ('isComposing' in native && native.isComposing === true);
}

export function fileTransferContainsFiles(types: Iterable<string>, fileCount: number): boolean {
  return fileCount > 0 || Array.from(types).includes('Files');
}

/**
 * The composer's text surface, seen by the hooks that own draft persistence and
 * prompt-history recall.
 *
 * Those hooks used to hold an `HTMLTextAreaElement` ref and poke `.value` /
 * `.setSelectionRange()` directly. The input is now Astryx's contentEditable
 * `ChatComposerInput`, whose value is React state and whose caret is owned by
 * the component — so they talk to this two-method port instead and stay free
 * of any DOM shape.
 */
export interface ComposerTextPort {
  /** The serialized draft text, tokens included. */
  getValue(): string;
  /** Replace the draft text without moving focus. */
  setValue(value: string): void;
}

/**
 * True when the caret is collapsed at the very start of `root`'s content, i.e.
 * the position where Backspace should eat the last staged Skill instead of a
 * character. The textarea equivalent was `selectionStart === selectionEnd === 0`.
 */
export function isCaretAtContentStart(root: Node, selection: Selection | null): boolean {
  if (!selection || !selection.isCollapsed || selection.rangeCount === 0) return false;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.startContainer)) return false;
  const probe = range.cloneRange();
  probe.selectNodeContents(root);
  probe.setEnd(range.startContainer, range.startOffset);
  return probe.toString().length === 0;
}

/**
 * Case-insensitive AND-of-substring matcher shared by the file re-filter and
 * the skill filter: every whitespace-separated token in `query` must appear
 * somewhere in `text`. An empty query matches everything (shows the full list).
 */
export function mentionQueryMatches(query: string, text: string): boolean {
  const haystack = text.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .every((token) => haystack.includes(token));
}

/** Normalize `/skill:<query>` and bare `/<query>` into the same Skill search query. */
export function skillMentionQuery(query: string): string {
  return query.toLowerCase().startsWith('skill:') ? query.slice('skill:'.length) : query;
}

export interface ChatInputActionOwner<ActionId> {
  readonly pending: ActionId | null;
  run<Result>(actionId: ActionId, action: () => Promise<Result>): Promise<Result | undefined>;
  reset(): void;
}

export function createChatInputActionOwner<ActionId>(
  onPendingChange: (action: ActionId | null) => void,
): ChatInputActionOwner<ActionId> {
  let pending: ActionId | null = null;
  let generation = 0;
  return {
    get pending() {
      return pending;
    },
    async run<Result>(actionId: ActionId, action: () => Promise<Result>): Promise<Result | undefined> {
      if (pending !== null) return undefined;
      const ownedGeneration = ++generation;
      pending = actionId;
      onPendingChange(actionId);
      try {
        return await action();
      } finally {
        if (generation === ownedGeneration && pending === actionId) {
          pending = null;
          onPendingChange(null);
        }
      }
    },
    reset() {
      generation += 1;
      pending = null;
    },
  };
}
