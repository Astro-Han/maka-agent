import { strict as assert } from 'node:assert';
import { afterEach, before, describe, it } from 'node:test';
import { act, createElement, StrictMode, type ReactElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';

// #2030: every footer action in the transcript (chat-turn.tsx) wraps its button
// in an Astryx `Tooltip`. Astryx builds the trigger ref from a fresh closure and
// a fresh layer object on every render, and `Tooltip`'s layout effects depend on
// that ref — so one parent re-render rewrote `aria-describedby` and the inline
// `anchor-name` on every tooltip trigger on the page, whether or not the tooltip
// itself had changed. During a streamed answer that is thousands of attribute
// writes per turn against untouched history.
//
// `patches/@astryxdesign+core+0.2.0.patch` splits attachment in two: the anchor
// name and `aria-describedby` are keyed on the trigger ELEMENT (idempotent, so
// an unchanged element is skipped), while the event listeners stay keyed on the
// CLOSURE that installed them (so they keep seeing current props, and can be
// removed with the same function identities that added them).
//
// This test drives the real component through a DOM that records attribute
// writes, style writes, and listener bindings, so it goes red the moment the
// patch stops applying or regresses either half.

type WriteLog = { attributes: string[]; styles: string[] };
type ActGlobal = typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };
type FakeDom = {
  document: Document;
  window: Window & typeof globalThis;
};

const cleanupTasks: Array<() => void> = [];

// Bound at the bottom of this file, once a `window` exists. See the note there.
let Tooltip: typeof import('@astryxdesign/core/Tooltip').Tooltip;

afterEach(() => {
  while (cleanupTasks.length > 0) cleanupTasks.pop()?.();
});

describe('Astryx tooltip trigger stability', () => {
  it('writes nothing to the trigger when the parent re-renders for an unrelated reason', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'first render' }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger, 'the tooltip must attach to the button it wraps');
    assert.ok(
      trigger.getAttribute('aria-describedby'),
      'the first mount must describe the trigger',
    );
    assert.ok(
      trigger.writes.styles.some((name) => name === 'anchorName'),
      'the first mount must give the trigger an anchor name',
    );

    trigger.writes.attributes.length = 0;
    trigger.writes.styles.length = 0;

    await render(root, harness({ label: 'unrelated parent update' }));
    await render(root, harness({ label: 'another unrelated parent update' }));

    assert.deepEqual(
      trigger.writes.attributes,
      [],
      'an unrelated re-render must not touch the trigger attributes',
    );
    assert.deepEqual(
      trigger.writes.styles,
      [],
      'an unrelated re-render must not rewrite the trigger anchor name',
    );
    assert.ok(
      trigger.getAttribute('aria-describedby'),
      'the trigger must still be described after the re-renders',
    );
  });

  // The other half of the contract, and the one a ref-identity-keyed effect
  // gets wrong the moment the identity stops churning: when the trigger really
  // does change, the tooltip has to follow it.
  it('hands the anchor over when the trigger element is replaced', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'button trigger', trigger: 'button' }));
    const replaced = findTrigger(container, 'BUTTON');
    assert.ok(replaced, 'the first trigger must mount');

    await render(root, harness({ label: 'link trigger', trigger: 'a' }));
    const adopted = findTrigger(container, 'A');
    assert.ok(adopted, 'the replacement trigger must mount');

    assert.ok(
      adopted.getAttribute('aria-describedby'),
      'the replacement trigger must be described',
    );
    assert.ok(anchorNameOf(adopted), 'the replacement trigger must carry the anchor name');
    assert.equal(
      anchorNameOf(replaced),
      '',
      'the detached trigger must not keep an anchor name pointing at a live layer',
    );
    assert.equal(
      replaced.getAttribute('aria-describedby'),
      null,
      'the detached trigger must be left as it was found',
    );
  });

  it('attaches to a child that arrives after the first render', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'no trigger yet', trigger: 'none' }));
    assert.equal(findTrigger(container, 'BUTTON'), undefined);

    await render(root, harness({ label: 'trigger arrives', trigger: 'button' }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger, 'the late child must mount');
    assert.ok(
      trigger.getAttribute('aria-describedby'),
      'a child that arrives late must still be described',
    );
    assert.ok(
      anchorNameOf(trigger),
      'a child that arrives late must still carry the anchor name',
    );
  });

  // Opening the tooltip is a real state change, so `useLayer` hands consumers a
  // new layer object — but the trigger element has not moved, so nothing on it
  // may be rewritten, and its anchor must survive.
  it('keeps the trigger untouched across a controlled open', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'closed', isOpen: false }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger, 'the trigger must mount');
    const anchorName = anchorNameOf(trigger);
    assert.ok(anchorName, 'the trigger must carry an anchor name while closed');

    trigger.writes.attributes.length = 0;
    trigger.writes.styles.length = 0;

    await render(root, harness({ label: 'open', isOpen: true }));

    assert.deepEqual(trigger.writes.attributes, [], 'opening must not rewrite trigger attributes');
    assert.deepEqual(trigger.writes.styles, [], 'opening must not rewrite the trigger anchor name');
    assert.equal(anchorNameOf(trigger), anchorName, 'the open layer must still be anchored');
    assert.ok(
      trigger.getAttribute('aria-describedby'),
      'the open layer must still describe its trigger',
    );
  });

  it('renders the trigger inline and describes it through JSX for text-only children', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'text child', trigger: 'text' }));
    const span = findTrigger(container, 'SPAN');
    assert.ok(span, 'a text-only child must be wrapped in a span trigger');
    assert.ok(
      span.getAttribute('aria-describedby'),
      'the text-only trigger carries aria-describedby through JSX',
    );
    assert.ok(
      span.boundTypes().length > 0,
      'the text-only trigger still gets its interaction listeners',
    );
  });
});

// The listeners are the half that an element-keyed attachment silently drops:
// nothing about `anchor-name` or `aria-describedby` would fail, and the tooltip
// would simply never open in a real browser. Assert them directly.
describe('Astryx tooltip trigger listeners', () => {
  const expected = ['mouseenter', 'mouseleave', 'pointerdown', 'focusin', 'focusout'];

  it('binds the full hover and focus set to the trigger', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'first render' }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);
    assert.deepEqual(
      trigger.boundTypes(),
      expected,
      'a focusable trigger must carry every hover and focus listener',
    );
  });

  it('does not accumulate listeners across unrelated re-renders', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'first render' }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);

    await render(root, harness({ label: 'second render' }));
    await render(root, harness({ label: 'third render' }));

    assert.deepEqual(
      trigger.boundTypes(),
      expected,
      'a re-render may rebind listeners, but the net bound set must not grow',
    );
  });

  it('moves every listener when the trigger element is replaced', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness({ label: 'button trigger', trigger: 'button' }));
    const replaced = findTrigger(container, 'BUTTON');
    assert.ok(replaced);

    await render(root, harness({ label: 'link trigger', trigger: 'a' }));
    const adopted = findTrigger(container, 'A');
    assert.ok(adopted);

    assert.deepEqual(replaced.boundTypes(), [], 'the detached trigger must keep no listeners');
    assert.deepEqual(
      adopted.boundTypes(),
      expected,
      'the replacement trigger must carry every listener',
    );
  });

  // The listeners close over `isEnabled`, `delay`, `hideDelay`, `focusTrigger`
  // and `onOpenChange`. Keying them on the trigger element alone freezes the
  // closure from the mounting render, so a prop change stops taking effect and
  // a disabled tooltip keeps opening on hover.
  it('honours a prop change that only the listener closure can see', async () => {
    const { root, container } = installReactRenderer();
    const opened: boolean[] = [];
    const onOpenChange = (next: boolean): void => {
      opened.push(next);
    };

    await render(root, harness({ label: 'enabled', isEnabled: true, onOpenChange }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);

    await render(root, harness({ label: 'disabled', isEnabled: false, onOpenChange }));

    await hover(trigger);
    assert.deepEqual(
      opened,
      [],
      'a tooltip disabled after mount must not open on hover',
    );

    await render(root, harness({ label: 're-enabled', isEnabled: true, onOpenChange }));
    await hover(trigger);
    assert.deepEqual(
      opened,
      [true],
      'a tooltip re-enabled after mount must open on hover again',
    );
  });

  // `removeEventListener` only matches the exact function that was added, so the
  // unmount cleanup has to run through the same closure that bound them. Holding
  // only the newest ref makes every removal a silent no-op.
  it('removes every listener it added when the tooltip unmounts', async () => {
    const { root, container } = installReactRenderer();
    const anchor = { current: null as unknown as FakeElement | null };

    await render(root, siblingHarness({ label: 'mounted', anchor, mounted: true }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);
    assert.deepEqual(trigger.boundTypes(), expected, 'the sibling trigger must be bound');

    // Several re-renders first: each one hands `Tooltip` a fresh interaction
    // closure, so the cleanup must track which one actually holds the listeners.
    await render(root, siblingHarness({ label: 'churn 1', anchor, mounted: true }));
    await render(root, siblingHarness({ label: 'churn 2', anchor, mounted: true }));

    await render(root, siblingHarness({ label: 'unmounted', anchor, mounted: false }));

    assert.ok(findTrigger(container, 'BUTTON'), 'the external trigger must outlive the tooltip');
    assert.deepEqual(
      trigger.boundTypes(),
      [],
      'unmounting the tooltip must leave no listener behind on a trigger it does not own',
    );
  });
});

// Sibling mode (`anchorRef`) attaches to an element the tooltip does not render
// and therefore does not unmount. Everything it wrote has to come back off, and
// nothing anybody else wrote may come off with it.
describe('Astryx tooltip sibling-mode detachment', () => {
  it('leaves an external trigger exactly as it found it', async () => {
    const { root, container } = installReactRenderer();
    const anchor = { current: null as unknown as FakeElement | null };

    await render(root, siblingHarness({ label: 'mounted', anchor, mounted: true }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);
    const describedBy = trigger.getAttribute('aria-describedby');
    assert.ok(describedBy?.includes('external-note'), 'the app id must be preserved');
    assert.notEqual(describedBy, 'external-note', 'the tooltip must add its own id');
    assert.ok(anchorNameOf(trigger), 'the external trigger must carry the anchor name');

    await render(root, siblingHarness({ label: 'unmounted', anchor, mounted: false }));

    assert.ok(findTrigger(container, 'BUTTON'), 'the external trigger must outlive the tooltip');
    assert.equal(
      trigger.getAttribute('aria-describedby'),
      'external-note',
      'unmounting must remove only the tooltip id, leaving the app id in place',
    );
    assert.equal(
      anchorNameOf(trigger),
      '',
      'unmounting must take the anchor name off an element it does not own',
    );
  });

  // Detaching by restoring the `aria-describedby` snapshot taken at attach time
  // passes as long as tooltips unmount in reverse order. Unmount the one that
  // attached FIRST and the snapshot is stale: it predates the second tooltip's
  // id, so restoring it silently un-describes a tooltip that is still mounted.
  it('keeps a sibling tooltip described when the tooltip that attached first unmounts', async () => {
    const { root, container } = installReactRenderer();
    const anchor = { current: null as unknown as FakeElement | null };

    await render(root, sharedTriggerHarness({ anchor, first: true }));
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger);
    const both = trigger.getAttribute('aria-describedby')?.split(' ') ?? [];
    assert.equal(both.length, 3, 'both tooltips plus the app id must describe the trigger');
    assert.equal(both[0], 'external-note', 'the app id must come first');
    const survivingId = both[2] as string;

    await render(root, sharedTriggerHarness({ anchor, first: false }));

    const remaining = trigger.getAttribute('aria-describedby')?.split(' ') ?? [];
    assert.ok(remaining.includes('external-note'), 'the app id must survive');
    assert.ok(
      remaining.includes(survivingId),
      'the still-mounted tooltip must still describe the trigger',
    );
    assert.equal(
      remaining.length,
      2,
      'unmounting one tooltip must drop only its own id',
    );
  });
});

// The renderer mounts under StrictMode (apps/desktop/src/renderer/app.tsx), which
// double-invokes mount effects: every effect is run, cleaned up, and run again.
// An attachment that is not idempotent under that survives every other test here.
describe('Astryx tooltip under StrictMode', () => {
  it('ends a double-invoked mount fully attached exactly once', async () => {
    const { root, container } = installReactRenderer();

    await render(
      root,
      createElement(StrictMode, null, harness({ label: 'strict mount' })),
    );
    const trigger = findTrigger(container, 'BUTTON');
    assert.ok(trigger, 'the trigger must mount under StrictMode');

    assert.deepEqual(
      trigger.boundTypes(),
      ['mouseenter', 'mouseleave', 'pointerdown', 'focusin', 'focusout'],
      'a double-invoked mount must leave exactly one set of listeners',
    );
    const describedBy = trigger.getAttribute('aria-describedby');
    assert.ok(describedBy, 'the trigger must be described under StrictMode');
    assert.equal(
      describedBy.split(' ').length,
      1,
      'a double-invoked mount must not describe the trigger twice',
    );
    assert.ok(anchorNameOf(trigger), 'the trigger must carry the anchor name under StrictMode');
  });
});

function harness(options: {
  label: string;
  trigger?: 'button' | 'a' | 'text' | 'none';
  isOpen?: boolean;
  isEnabled?: boolean;
  onOpenChange?: (open: boolean) => void;
}): ReactElement {
  const trigger = options.trigger ?? 'button';
  return createElement(
    'div',
    null,
    createElement('p', null, options.label),
    createElement(Tooltip, {
      content: 'Copy',
      isOpen: options.isOpen,
      isEnabled: options.isEnabled,
      delay: 0,
      onOpenChange: options.onOpenChange,
      children:
        trigger === 'none'
          ? null
          : trigger === 'text'
            ? 'Copy'
            : createElement(trigger, {}, 'Copy'),
    }),
  );
}

// Sibling mode: the trigger is rendered by the app, and the tooltip attaches to
// it through `anchorRef`. The tooltip can unmount while the trigger lives on.
function siblingHarness(options: {
  label: string;
  anchor: { current: FakeElement | null };
  mounted: boolean;
}): ReactElement {
  return createElement(
    'div',
    null,
    createElement('p', null, options.label),
    createElement(
      'button',
      {
        ref: (el: unknown) => {
          options.anchor.current = el as FakeElement | null;
        },
        'aria-describedby': 'external-note',
      },
      'Copy',
    ),
    options.mounted
      ? createElement(Tooltip, {
          content: 'Copy',
          delay: 0,
          anchorRef: options.anchor as never,
        })
      : null,
  );
}

// Two tooltips on one external trigger. `first` is the one that attached first,
// so its `aria-describedby` snapshot predates the second one's id — unmounting
// it is what tells a snapshot-restore apart from a remove-my-own-id.
function sharedTriggerHarness(options: {
  anchor: { current: FakeElement | null };
  first: boolean;
}): ReactElement {
  return createElement(
    'div',
    null,
    createElement(
      'button',
      {
        ref: (el: unknown) => {
          options.anchor.current = el as FakeElement | null;
        },
        'aria-describedby': 'external-note',
      },
      'Copy',
    ),
    options.first
      ? createElement(Tooltip, {
          key: 'first',
          content: 'Copy',
          delay: 0,
          anchorRef: options.anchor as never,
        })
      : null,
    createElement(Tooltip, {
      key: 'second',
      content: 'Also copy',
      delay: 0,
      anchorRef: options.anchor as never,
    }),
  );
}

// Drive a real hover through the listeners the trigger actually carries, then
// let the (zero) show delay elapse.
async function hover(trigger: FakeElement): Promise<void> {
  await act(async () => {
    trigger.dispatch('mouseenter');
    await new Promise((resolve) => setTimeout(resolve, 5));
  });
}

function anchorNameOf(element: FakeElement): string | undefined {
  return (element.style as unknown as Record<string, string | undefined>).anchorName;
}

function findTrigger(node: FakeElement, tagName: string): FakeElement | undefined {
  if (node.tagName === tagName) return node;
  for (const child of node.childNodes) {
    if (!(child instanceof FakeElement)) continue;
    const found = findTrigger(child, tagName);
    if (found) return found;
  }
  return undefined;
}

function installReactRenderer(): { root: Root; container: FakeElement } {
  installFakeDom();
  const container = new FakeElement('div', document);
  const root = createRoot(container as unknown as Element);
  cleanupTasks.push(() => {
    act(() => root.unmount());
  });
  return { root, container };
}

async function render(root: Root, element: ReactElement): Promise<void> {
  await act(async () => {
    root.render(element);
  });
}

function installFakeDom(): void {
  const previousDocument = globalThis.document;
  const previousWindow = globalThis.window;
  const previousHTMLElement = globalThis.HTMLElement;
  const previousHTMLIFrameElement = globalThis.HTMLIFrameElement;
  const previousActEnvironment = (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT;
  applyFakeDom(createFakeDom());
  (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = true;
  cleanupTasks.push(() => {
    globalThis.document = previousDocument;
    globalThis.window = previousWindow;
    globalThis.HTMLElement = previousHTMLElement;
    globalThis.HTMLIFrameElement = previousHTMLIFrameElement;
    (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = previousActEnvironment;
  });
}

function applyFakeDom(dom: FakeDom): void {
  globalThis.document = dom.document;
  globalThis.window = dom.window;
  globalThis.HTMLElement = FakeElement as unknown as typeof HTMLElement;
  globalThis.HTMLIFrameElement = dom.window.HTMLIFrameElement;
}

function createFakeDom(): FakeDom {
  const fakeDocument = createFakeDocument();
  const fakeWindow = {
    document: fakeDocument,
    addEventListener: () => {},
    removeEventListener: () => {},
    // Astryx suppresses tooltips where hover is simulated; the desktop app is
    // a pointer environment, so answer the way a real one does.
    matchMedia: (media: string) => ({ matches: false, media }),
    HTMLElement: FakeElement,
    HTMLIFrameElement: class HTMLIFrameElement {},
    setTimeout: (handler: () => void, ms: number) => setTimeout(handler, ms).unref(),
    clearTimeout: (handle: NodeJS.Timeout) => clearTimeout(handle),
  } as unknown as Window & typeof globalThis;
  Object.defineProperty(fakeDocument, 'defaultView', { value: fakeWindow });
  return { document: fakeDocument, window: fakeWindow };
}

function createFakeDocument(): Document {
  const fakeDocument = {
    nodeType: 9,
    addEventListener: () => {},
    removeEventListener: () => {},
    createElement(tagName: string) {
      return new FakeElement(tagName, fakeDocument as unknown as Document);
    },
    createElementNS(_namespace: string, tagName: string) {
      return new FakeElement(tagName, fakeDocument as unknown as Document);
    },
    createTextNode(text: string) {
      return new FakeText(text, fakeDocument as unknown as Document);
    },
  };
  Object.defineProperty(fakeDocument, 'documentElement', {
    value: new FakeElement('html', fakeDocument as unknown as Document),
  });
  return fakeDocument as unknown as Document;
}

class FakeElement {
  readonly attributes = new Map<string, string>();
  readonly childNodes: Array<FakeElement | FakeText> = [];
  readonly dataset: Record<string, string> = {};
  readonly namespaceURI = 'http://www.w3.org/1999/xhtml';
  readonly nodeName: string;
  readonly nodeType = 1;
  readonly style: CSSStyleDeclaration;
  readonly tagName: string;
  readonly writes: WriteLog = { attributes: [], styles: [] };
  // Listeners are tracked as (type, handler) pairs, matching the only thing
  // `removeEventListener` matches on. A cleanup that passes a different function
  // identity leaves the entry behind here exactly as it would in a browser.
  private readonly listeners: Array<{ type: string; handler: unknown }> = [];
  parentNode: FakeElement | null = null;

  constructor(tagName: string, readonly ownerDocument: Document) {
    this.tagName = tagName.toUpperCase();
    this.nodeName = this.tagName;
    const writes = this.writes;
    this.style = new Proxy(
      {
        setProperty(name: string, value: string) {
          writes.styles.push(name);
          (this as Record<string, unknown>)[name] = value;
        },
        removeProperty(name: string) {
          writes.styles.push(name);
          delete (this as Record<string, unknown>)[name];
        },
      } as Record<string, unknown>,
      {
        set(target, property, value) {
          if (typeof property === 'string') writes.styles.push(property);
          target[property as string] = value;
          return true;
        },
      },
    ) as unknown as CSSStyleDeclaration;
  }

  get firstElementChild(): FakeElement | null {
    return this.childNodes.find((node) => node instanceof FakeElement) ?? null;
  }

  get textContent(): string {
    return this.childNodes.map((node) => node.textContent).join('');
  }

  set textContent(value: string) {
    this.childNodes.splice(0);
    if (value !== '') this.appendChild(new FakeText(value, this.ownerDocument));
  }

  addEventListener(type: string, handler: unknown): void {
    this.listeners.push({ type, handler });
  }

  removeEventListener(type: string, handler: unknown): void {
    const index = this.listeners.findIndex(
      (entry) => entry.type === type && entry.handler === handler,
    );
    if (index >= 0) this.listeners.splice(index, 1);
  }

  /** The event types currently bound, in binding order, for assertions. */
  boundTypes(): string[] {
    return this.listeners.map((entry) => entry.type);
  }

  /** Invoke every handler bound for `type`, the way a dispatched event would. */
  dispatch(type: string): void {
    const event = { type, target: this } as unknown as Event;
    for (const entry of [...this.listeners]) {
      if (entry.type === type) (entry.handler as (e: Event) => void)(event);
    }
  }

  matches(selector: string): boolean {
    // The only selector Astryx tests against a trigger is ':focus-visible'.
    return selector === ':focus-visible';
  }

  appendChild<T extends FakeElement | FakeText>(node: T): T {
    this.childNodes.push(node);
    node.parentNode = this;
    return node;
  }

  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }

  hasAttribute(name: string): boolean {
    return this.attributes.has(name);
  }

  insertBefore<T extends FakeElement | FakeText>(node: T, before: FakeElement | FakeText | null): T {
    const index = before ? this.childNodes.indexOf(before) : -1;
    if (index < 0) return this.appendChild(node);
    this.childNodes.splice(index, 0, node);
    node.parentNode = this;
    return node;
  }

  removeAttribute(name: string): void {
    this.writes.attributes.push(name);
    this.attributes.delete(name);
  }

  removeChild<T extends FakeElement | FakeText>(node: T): T {
    const index = this.childNodes.indexOf(node);
    if (index >= 0) this.childNodes.splice(index, 1);
    node.parentNode = null;
    return node;
  }

  setAttribute(name: string, value: string): void {
    this.writes.attributes.push(name);
    this.attributes.set(name, value);
  }
}

class FakeText {
  readonly nodeName = '#text';
  readonly nodeType = 3;
  parentNode: FakeElement | null = null;

  constructor(public nodeValue: string, readonly ownerDocument: Document) {}

  get textContent(): string {
    return this.nodeValue;
  }

  set textContent(value: string) {
    this.nodeValue = value;
  }
}

// A `window` MUST exist before `@astryxdesign/core` is first imported, which is
// why the import is dynamic, and why it runs from `before` rather than at module
// scope: `node:test` starts the first test as soon as module evaluation yields,
// so a top-level `await import()` would lose the race.
//
// `useIsomorphicLayoutEffect` resolves ONCE, at module evaluation:
//
//   export const useIsomorphicLayoutEffect =
//     typeof window !== 'undefined' ? useLayoutEffect : useEffect;
//
// A static `import {Tooltip}` is hoisted above every statement in this file, so
// it would evaluate against a bare Node global with no `window` and silently
// bind `useEffect`. The patch's mechanism is a dependency-array-free *layout*
// effect, so exercising it under `useEffect` validates the wrong timing:
// passive effects flush after paint and in a different order relative to ref
// callbacks and child cleanup, which is exactly where an attachment bug hides.
before(async () => {
  applyFakeDom(createFakeDom());
  ({ Tooltip } = await import('@astryxdesign/core/Tooltip'));
});
