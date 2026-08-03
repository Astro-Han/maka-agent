import { strict as assert } from 'node:assert';
import { afterEach, describe, it } from 'node:test';
import { Tooltip } from '@astryxdesign/core/Tooltip';
import { act, createElement, type ReactElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';

// #2030: every footer action in the transcript (chat-turn.tsx) wraps its button
// in an Astryx `Tooltip`. Astryx builds the trigger ref from a fresh closure and
// a fresh layer object on every render, and `Tooltip`'s layout effect depends on
// that ref — so one parent re-render rewrote `aria-describedby` and the inline
// `anchor-name` on every tooltip trigger on the page, whether or not the tooltip
// itself had changed. During a streamed answer that is thousands of attribute
// writes per turn against untouched history.
//
// `patches/@astryxdesign+core+0.2.0.patch` stabilizes both identities. This
// test drives the real component through a DOM that records writes, so it goes
// red the moment the patch stops applying.

type WriteLog = { attributes: string[]; styles: string[] };
type ActGlobal = typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };

const cleanupTasks: Array<() => void> = [];

afterEach(() => {
  while (cleanupTasks.length > 0) cleanupTasks.pop()?.();
});

describe('Astryx tooltip trigger stability', () => {
  it('writes nothing to the trigger when the parent re-renders for an unrelated reason', async () => {
    const { root, container } = installReactRenderer();

    await render(root, harness('first render'));
    const trigger = findTrigger(container);
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

    await render(root, harness('unrelated parent update'));
    await render(root, harness('another unrelated parent update'));

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
});

function harness(label: string): ReactElement {
  return createElement(
    'div',
    null,
    createElement('p', null, label),
    createElement(Tooltip, {
      content: 'Copy',
      children: createElement('button', { type: 'button' }, 'Copy'),
    }),
  );
}

function findTrigger(node: FakeElement): FakeElement | undefined {
  if (node.tagName === 'BUTTON') return node;
  for (const child of node.childNodes) {
    if (!(child instanceof FakeElement)) continue;
    const found = findTrigger(child);
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
  globalThis.document = fakeDocument;
  globalThis.window = fakeWindow;
  globalThis.HTMLElement = FakeElement as unknown as typeof HTMLElement;
  globalThis.HTMLIFrameElement = fakeWindow.HTMLIFrameElement;
  (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = true;
  cleanupTasks.push(() => {
    globalThis.document = previousDocument;
    globalThis.window = previousWindow;
    globalThis.HTMLElement = previousHTMLElement;
    globalThis.HTMLIFrameElement = previousHTMLIFrameElement;
    (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = previousActEnvironment;
  });
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

  addEventListener(): void {}

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

  removeEventListener(): void {}

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
