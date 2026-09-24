<!--
  Licensed to the Apache Software Foundation (ASF) under one
  or more contributor license agreements.  See the NOTICE file
  distributed with this work for additional information
  regarding copyright ownership.  The ASF licenses this file
  to you under the Apache License, Version 2.0 (the
  "License"); you may not use this file except in compliance
  with the License.  You may obtain a copy of the License at

      http://www.apache.org/licenses/LICENSE-2.0

  Unless required by applicable law or agreed to in writing,
  software distributed under the License is distributed on an
  "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
  KIND, either express or implied.  See the License for the
  specific language governing permissions and limitations
  under the License.
-->

# Desktop Components

Phase 1 of the GPUI client: chat, sessions and settings, with the fewest components that make them usable.

## Direction

- The reference is Waku's desktop client (look, density, behavior), not a port of the Electron app or Astryx. Waku is GPL: we study how it looks and behaves and implement it ourselves; no code is copied or translated.
- Waku builds everything on raw GPUI, including its markdown renderer, text input, menus and scrollbar. We do not: those come from gpui-kit. We only write what gpui-kit lacks.
- Colors are Maka's own: graphite neutrals and the Maka blue as the single accent.

## Theme

One gpui-kit `ThemeConfig` for light and one for dark. No token system of our own.

- Surfaces: sidebar and canvas share one neutral; menus and dialogs use `popover`; code wells use `muted`.
- Text: `foreground` and `muted_foreground`.
- One border color; hover and selected are the same faint neutral wash; hover changes are instant.
- Radius: 8 for controls and rows, 12 for bubbles, composer, cards and dialogs, pill for round buttons.
- Arrow cursor everywhere, as native macOS apps do.
- Reading column: 720 px, centered, shared by transcript and composer.

## What gpui-kit already covers

| Need | gpui-kit |
|---|---|
| Transcript list that follows the tail, jump to latest | `MessageScroller` |
| Markdown, code highlighting, selection, stream fade | `TextView` (fade needs the throttle fix below) |
| User bubble | `Bubble` |
| Copy button with check feedback | `Clipboard` |
| Folding | `Collapsible` |
| One-line status with spinner or shimmer | `Marker`, `ShimmerText` |
| Composer text field that grows | `Textarea` with `appearance(false)` and `auto_grow` |
| Question interaction | `Questionnaire` (on gpui-kit main, not in 0.6.6) |
| Settings pages, forms, dialogs, toasts, menus, tooltips | `Settings`, `Form`, `Dialog`, `Notification`, `PopupMenu`, `Tooltip` |

gpui-kit has no tool-call, diff or terminal component.

## What we write

| Component | Look and behavior | Built from |
|---|---|---|
| Session row | two lines: name, then muted workspace and relative time; status dot for running or waiting; one wash for hover and selected | own `SidebarItem` |
| Tool row | one line: icon, verb, target, status (running, waiting, done, failed); expands to plain monospace output. Consecutive rows fold to "N commands"; a finished turn folds to "Worked for Ns" | `Collapsible` |
| Working line | three dots in a travelling wave plus elapsed time; also shows retry and failure with resume | own, timer-driven |
| Permission card | reason, command in a monospace well, deny and allow choices; pinned above the composer | `Button` |
| Composer card | 12 radius card with a hairline border, borderless textarea, toolbar row, round send button that becomes stop while a turn runs | `Textarea`, `Button` |
| Message hover footer | copy and time, shown only while the message is hovered | `Clipboard`, GPUI `group_hover` |

Everything else in the window is layout: transparent titlebar, sidebar with a hairline divider, user bubbles right-aligned at most 540 wide, assistant text without bubble or avatar.

Code blocks keep gpui-kit's corner copy button. The sidebar is a solid color: GPUI's macOS blur covers the whole window with one fixed material.

Later, not phase 1: diff coloring, session groups, queued messages, model chip, context meter, attachments, OAuth.

## Motion

Animations redraw on a ~30 fps timer and stop when they finish; nothing redraws at display rate. gpui-kit's `request_animation_frame` drivers (stream fade, `Spinner`, `ShimmerText`) hold a ProMotion window at 120 Hz while visible, so the working line uses our own timer, and the stream fade runs with a local patch until the upstream fix lands.

## Acceptance

Each written component appears in a gallery window (`maka-desktop --gallery`) in every state, light and dark; takes plain data, never protocol types; and causes no redraws while idle.

## Pitfall checklist

Problems Waku hit and solved, restated as checks. Waku started on gpui-component and later replaced it in the transcript and markdown, so the gpui-kit items below are unverified until a UI test proves them; a failing item is fixed upstream where possible, otherwise the component is written here. Each item becomes a test.

### Transcript list (gpui-kit `MessageScroller`)

- A row that has not been measured yet has unknown height, not zero. Reading it as zero makes the view jump between prompt and tail at stream speed and blinks the jump-to-latest button.
- While a turn runs, the sent prompt stays pinned at the top and the reply grows below it; before the first measurement a full viewport of end space is reserved so the prompt does not flash at the bottom.
- A user scroll stops following; scrolling back onto the tail resumes it; dragging the scrollbar stops it for the whole drag.
- Streaming remeasures only the last few rows; a width change remeasures all; finishing a turn replaces exactly the changed rows so the list never shows row zero for a frame.
- Expanding or collapsing a row first fixes the scroll position, so the header under the pointer does not move.

### Streaming markdown (gpui-kit `TextView`)

- Unclosed `**`, `` ` ``, `~~` and half-typed links are closed in a display copy of the tail only, never in the parsed document; code and math are never mended. Tested on every prefix of a corpus, and mending twice changes nothing.
- Each delta reparses from the last stable block, keeping the last two blocks open (a table row or inline image can reshape the previous block); a link reference definition forces full parses.
- The fade changes paint only (colors), never run lengths or fonts, so wrapping, selection and row height stay put. After a rewrite (`**bo` becoming bold) only the part past the common prefix fades; attaching to a stream in progress shows existing text at full opacity.
- Selection spans blocks, survives rows scrolling out of view, and starts continuation rows at their first glyph (soft-wrap boundaries are walked, not looked up by index). Copy yields rendered text.
- Streaming code is highlighted by a lexer that leaves the unfinished tail plain; highlighting never changes widths.
- A long live reasoning block renders only its tail window.

### Composer input (gpui-kit `Textarea`)

- IME ranges relative to marked text are computed in UTF-16 before converting to bytes; tested with Chinese before the caret. Enter during composition commits the composition, it does not submit. Autocomplete and drafts ignore marked text.
- Enter submits; Shift-, Ctrl- and Alt-Enter insert a newline. Modified deletes are bound explicitly, since GPUI swallows unbound chords.
- Undo groups by gesture: paste, cut and completion start a step, a composition is one step, setting the text programmatically clears history, undo is refused while composing.
- Owners react to an edited event, not to every notify (the caret blink notifies too).
- Pasting follows clipboard order: image first attaches, file paths first attach, text first pastes text.

### Overlays and focus

- Focus a newly opened deferred overlay two frames later, guarded by a generation counter; focusing earlier silently fails.
- A click on the trigger is exempt from click-outside dismissal, or the menu closes and reopens. Space typed inside a popover's field must not reach the trigger.
- When the focused element disappears, focus returns to the composer.

### Our components

- **Tool rows**: a group stays live while it is the last block of the running turn and no answer text follows; live, its header shows the latest action, settled, a count summary. A finished turn folds all work into one "Worked for Ns" line above the answer.
- **Working line**: appears as soon as the prompt lands, before any output.
- **Hover footer**: its own row, so later tool activity cannot strand it; hidden actions keep their space so hover never changes height; hovering any row of a response shows it, the prompt does not. Copy takes the visible answer only. The copied check mark uses a generation counter so an older timer cannot clear a newer click.
- **Permission card**: outside the list, above the composer. Keyboard reachable (Waku's is click-only; we do better). Requests from a turn that already ended are ignored.
- **Session row**: fixed height including the gap, so scroll-to-selected works before rows are measured. Order by last reply, not name. A click selects immediately while the session loads. Escape in the rename field does not stop the turn.
- **Relative time**: one timer set to the next moment any visible label changes (every second while busy), not a periodic refresh.
- **Window chrome**: window drag starts on the first move after mouse down, so clicks and double-clicks still work; double-click follows the system setting.
- **Diff and terminal** (later): diff built when expanded, dropped when collapsed, capped with a note; terminal output strips ANSI, including sequences split across chunks.

### Frame budget

- Batch stream deltas for ~120 ms into one update, one notify, one remeasure, for every delta kind including reasoning.
- No animation redraws at display rate; animations use a low-rate timer that stops when idle, and stay still under reduce motion.
- Notify the smallest enclosing view; never refresh the whole window.
- A cached view needs a flex parent, or its height collapses to zero.
