/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

import { Terminal } from '@xterm/headless';
import { Unicode11Addon } from '@xterm/addon-unicode11';

// Only the headless parser lives here. Rust owns resource lifecycle and every
// native effect. No model code, filesystem, network or credentials enter this isolate.
globalThis.createTerminal = ({ cols, rows }) => {
  const terminal = new Terminal({
    cols,
    rows,
    scrollback: 500,
    allowProposedApi: true,
    scrollOnEraseInDisplay: false,
    windowOptions: {},
    logLevel: 'off',
  });
  terminal.loadAddon(new Unicode11Addon());
  terminal.unicode.activeVersion = '11';
  let visible = true,
    mouseEncoding = 'default',
    lastAlternateScreen;
  let truncated = false,
    atHistoryLimit = false,
    suppressScroll = false;
  let replyBytes = 0,
    replies,
    failure;
  let expansion = 0;
  const alternate = (params) => params.some((p) => [47, 1047, 1049].includes(p));
  const flat = (params) => params.flatMap((p) => (Array.isArray(p) ? p : [p]));
  terminal.onData((data) => {
    if (replies === undefined) {
      failure ||= new Error('Terminal reply escaped the parser write boundary');
      return;
    }
    replyBytes += Deno.core.byteLength(data);
    if (replyBytes > 1024 * 1024) {
      failure ||= new Error('Terminal protocol reply budget exceeded');
      return;
    }
    replies += data;
  });
  for (const id of [0, 1, 2, 7, 8, 9, 52, 777]) {
    terminal.parser.registerOscHandler(id, () => true);
  }
  // These handlers loop or allocate directly from Ps. Input byte and V8 heap
  // limits do not bound REP's external ArrayBuffer memory or repeated tab work.
  for (const final of ['I', 'Z', 'L', 'M', 'S', 'T', 'b']) {
    terminal.parser.registerCsiHandler({ final }, (params) => {
      const count = flat(params)[0] || 1;
      let unit = ['L', 'M', 'S', 'T'].includes(final) ? terminal.cols : 1;
      if (final === 'b') {
        const active = terminal.buffer.active;
        const line = active.getLine(active.baseY + active.cursorY);
        // Include both cells of a wide character and combined Unicode clusters.
        for (const x of [active.cursorX - 1, active.cursorX - 2]) {
          if (x >= 0) unit = Math.max(unit, line?.getCell(x)?.getChars().length || 1);
        }
      }
      expansion += count * unit;
      if (expansion <= 64 * 1024 && !failure) return false;
      failure ||= new Error('Terminal escape expansion budget exceeded');
      return true;
    });
  }
  terminal.onScroll(() => {
    if (terminal.buffer.active.type !== 'normal') return;
    const full = terminal.buffer.normal.baseY >= 500;
    if (full && atHistoryLimit && !suppressScroll) truncated = true;
    atHistoryLimit = full;
    suppressScroll = false;
  });
  terminal.parser.registerCsiHandler({ prefix: '?', final: 'h' }, (params) => {
    const values = flat(params);
    if (alternate(values)) {
      lastAlternateScreen = undefined;
      suppressScroll = true;
    }
    if (values.includes(25)) visible = true;
    for (const value of values) {
      if (value === 1006) mouseEncoding = 'sgr';
      if (value === 1016) mouseEncoding = 'sgr_pixels';
    }
    return false;
  });
  terminal.parser.registerCsiHandler({ prefix: '?', final: 'l' }, (params) => {
    const values = flat(params);
    if (alternate(values)) {
      if (terminal.buffer.active.type === 'alternate') {
        const retained = render(terminal.buffer.active, terminal.rows, true);
        lastAlternateScreen = retained.screen || undefined;
        truncated ||= retained.truncated;
      }
      suppressScroll = true;
    }
    if (values.includes(25)) visible = false;
    if (values.includes(1006) || values.includes(1016)) mouseEncoding = 'default';
    return false;
  });
  terminal.parser.registerEscHandler({ final: 'c' }, () => {
    visible = true;
    mouseEncoding = 'default';
    lastAlternateScreen = undefined;
    return false;
  });
  terminal.parser.registerCsiHandler({ intermediates: '!', final: 'p' }, () => {
    visible = true;
    return false;
  });
  const write = async (data) => {
    expansion = 0;
    replies = '';
    await new Promise((resolve) => terminal.write(data, resolve));
    if (failure) throw failure;
    suppressScroll = false;
    const result = replies;
    replies = undefined;
    return result;
  };
  return {
    write,
    resize(size) {
      terminal.resize(size.cols, size.rows);
    },
    async resetAfterGap() {
      // A dropped escape terminator can leave the parser inside OSC/DCS.
      // RIS resets parser state as well as the screen; terminal.reset() does not.
      await write('\x1bc');
      truncated = true;
      atHistoryLimit = false;
      suppressScroll = false;
    },
    snapshot() {
      if (failure) throw failure;
      const active = terminal.buffer.active;
      const output = render(active, terminal.rows, false);
      return {
        screen: output.screen,
        scrollback: output.scrollback,
        ...(lastAlternateScreen === undefined ? {} : { lastAlternateScreen }),
        size: { cols: terminal.cols, rows: terminal.rows },
        cursor: { x: active.cursorX, y: active.cursorY, visible },
        alternateScreen: active.type === 'alternate',
        truncated: truncated || output.truncated,
        input: {
          applicationCursorKeysMode: terminal.modes.applicationCursorKeysMode,
          mouseTrackingMode: terminal.modes.mouseTrackingMode,
          mouseEncoding,
        },
      };
    },
    dispose() {
      terminal.dispose();
    },
  };
};

function render(buffer, rows, screenOnly) {
  const start = buffer.type === 'normal' ? buffer.baseY : buffer.viewportY;
  const lines = [];
  for (let i = screenOnly ? start : 0; i < Math.min(buffer.length, start + rows); i++) {
    const line = buffer.getLine(i);
    lines.push({ text: line.translateToString(true), wrapped: line.isWrapped, index: i });
  }
  let truncated = false;
  while (lines[0]?.wrapped) {
    lines.shift();
    truncated = true;
  }
  const screen = Array(rows).fill(''),
    scrollback = [];
  for (const line of lines) {
    if (line.index >= start) screen[line.index - start] = line.text;
    else if (line.wrapped && scrollback.length) scrollback[scrollback.length - 1] += line.text;
    else scrollback.push(line.text);
  }
  while (screen.at(-1) === '') screen.pop();
  return { screen: screen.join('\n'), scrollback: scrollback.join('\n'), truncated };
}
