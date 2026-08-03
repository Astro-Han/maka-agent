// Unit test for the `maka.cu/2` parsing layer: the key grammar the host owns
// (§6.4) and the readers that refuse rather than default (§1.3, §5.2). No child
// process is involved — these are pure functions over the wire's shapes.
//
// Run (from repo root), after @maka/core + @maka/runtime are built:
//   npm --workspace @maka/computer-use run test
import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  MakaCuProtocolViolation,
  parseMakaCuKeyChord,
  readElement,
  readSnapshot,
  readWindow,
} from '../maka-cu-protocol.js';

const DIGEST = `sha256:${'a1'.repeat(32)}`;

function element(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    token: 'el_2',
    parentToken: 'el_1',
    depth: 1,
    role: 'AXButton',
    label: 'Send',
    enabled: true,
    focused: false,
    selected: null,
    frame: { x: 20, y: 40, width: 72, height: 28 },
    actions: ['press'],
    digest: DIGEST,
    truncated: [],
    ...overrides,
  };
}

function snapshot(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    snapshotId: 'snap_1',
    capturedAt: 1753574400123,
    target: {
      pid: 4711,
      windowId: 90210,
      appId: 'com.apple.Notes',
      appName: 'Notes',
      title: 'Untitled',
      bounds: { x: 0, y: 25, width: 1200, height: 800 },
      layer: 0,
      zIndex: 3,
    },
    windowDigest: DIGEST,
    focusedElementToken: null,
    selectedText: null,
    image: null,
    displays: [],
    obscuringRects: [],
    elements: [element()],
    truncated: { elements: false, depth: false },
    ...overrides,
  };
}

describe('maka-cu key grammar (§6.4)', () => {
  it('parses the combinations Maka callers actually hold', () => {
    assert.deepEqual(parseMakaCuKeyChord('cmd+a'), { key: 'a', modifiers: ['command'] });
    assert.deepEqual(parseMakaCuKeyChord('shift+Tab'), { key: 'Tab', modifiers: ['shift'] });
    assert.deepEqual(parseMakaCuKeyChord('Return'), { key: 'Return', modifiers: [] });
    assert.deepEqual(parseMakaCuKeyChord('a'), { key: 'a', modifiers: [] });
    // A trailing empty segment means the key is literally `+`.
    assert.deepEqual(parseMakaCuKeyChord('cmd++'), { key: '+', modifiers: ['command'] });
    assert.deepEqual(parseMakaCuKeyChord('+'), { key: '+', modifiers: [] });
  });

  it('maps aliases case-insensitively onto the wire vocabulary', () => {
    assert.deepEqual(parseMakaCuKeyChord('CTRL+esc'), { key: 'Escape', modifiers: ['control'] });
    assert.deepEqual(parseMakaCuKeyChord('opt+pgdn'), { key: 'PageDown', modifiers: ['option'] });
    assert.deepEqual(parseMakaCuKeyChord('meta+arrowup'), { key: 'Up', modifiers: ['command'] });
    assert.deepEqual(parseMakaCuKeyChord('enter'), { key: 'Return', modifiers: [] });
    // A single printable character keeps its case; only the aliases fold.
    assert.deepEqual(parseMakaCuKeyChord('A'), { key: 'A', modifiers: [] });
  });

  it('collapses a duplicated modifier and keeps first-seen order', () => {
    assert.deepEqual(parseMakaCuKeyChord('cmd+command+shift+a'), {
      key: 'a',
      modifiers: ['command', 'shift'],
    });
  });

  it('refuses `delete` and `del`, which name two different destructive keys', () => {
    // The Mac legend on backspace, the forward delete in xdotool; picking
    // either deletes the wrong character.
    assert.equal(parseMakaCuKeyChord('delete'), undefined);
    assert.equal(parseMakaCuKeyChord('del'), undefined);
    assert.deepEqual(parseMakaCuKeyChord('Backspace'), { key: 'Backspace', modifiers: [] });
    assert.deepEqual(parseMakaCuKeyChord('forwarddelete'), {
      key: 'ForwardDelete',
      modifiers: [],
    });
  });

  it('refuses everything the grammar cannot express', () => {
    for (const input of ['', 'cmd+', 'a+b', 'hyper+a', 'cmd++a', '  ', 'Enter+', 'Delete']) {
      assert.equal(parseMakaCuKeyChord(input), undefined, input);
    }
    // Space is the only spelling of U+0020, so the bare character is not a key.
    assert.equal(parseMakaCuKeyChord(' '), undefined);
    assert.deepEqual(parseMakaCuKeyChord('space'), { key: 'Space', modifiers: [] });
  });
});

describe('maka-cu readers refuse rather than default', () => {
  it('reads the element frame into a window-local field', () => {
    // §5.3: the space is carried by the name, so nothing can write it into
    // `CuObservedElement.frame` (screen points) without going through the one
    // conversion.
    assert.deepEqual(readElement('observe', element()).frameInWindow, {
      x: 20,
      y: 40,
      width: 72,
      height: 28,
    });
  });

  it('refuses a hash that is not written the one declared way (§1.3)', () => {
    assert.throws(
      () => readElement('observe', element({ digest: 'a1'.repeat(32) })),
      MakaCuProtocolViolation,
    );
    assert.throws(
      () => readSnapshot('observe', snapshot({ windowDigest: 'a1'.repeat(32) })),
      MakaCuProtocolViolation,
    );
  });

  it('refuses a declared array that is absent or malformed (§5.2)', () => {
    for (const missing of ['displays', 'obscuringRects', 'elements']) {
      const value = snapshot();
      delete value[missing];
      assert.throws(() => readSnapshot('observe', value), MakaCuProtocolViolation, missing);
    }
  });

  it('refuses a non-string token where the wire declares string-or-null (§5.2)', () => {
    // Coercing this to `null` would silently reparent the element to the root.
    assert.throws(
      () => readElement('observe', element({ parentToken: 7 })),
      MakaCuProtocolViolation,
    );
    assert.throws(
      () => readSnapshot('observe', snapshot({ focusedElementToken: 7 })),
      MakaCuProtocolViolation,
    );
    assert.throws(
      () => readElement('observe', element({ selected: 'yes' })),
      MakaCuProtocolViolation,
    );
    assert.equal(readElement('observe', element({ selected: null })).selected, null);
  });

  it('refuses an app string that is not on the wire (§5.1)', () => {
    const value = snapshot();
    delete (value.target as Record<string, unknown>).appId;
    assert.throws(() => readSnapshot('observe', value), MakaCuProtocolViolation);
  });

  it('refuses a text field that is present but is not text (§5.2)', () => {
    // An element whose label failed to parse is not an element without a label.
    assert.throws(() => readElement('observe', element({ label: 12 })), MakaCuProtocolViolation);
    assert.equal(readElement('observe', element({ label: null })).label, undefined);
    // An empty value is a fact about the field, not an absent one.
    assert.equal(readElement('observe', element({ value: '' })).value, '');
  });

  it('refuses a window entry missing a field the host sorts on (§5.4)', () => {
    const window = {
      pid: 4711,
      windowId: 90210,
      appId: 'com.apple.Notes',
      layer: 0,
      zIndex: 3,
      onScreen: true,
    };
    assert.deepEqual(readWindow('window.list', window), window);
    for (const missing of ['appId', 'zIndex', 'onScreen', 'layer']) {
      const broken: Record<string, unknown> = { ...window };
      delete broken[missing];
      assert.throws(() => readWindow('window.list', broken), MakaCuProtocolViolation, missing);
    }
  });
});
