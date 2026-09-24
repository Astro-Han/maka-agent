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

Problems Waku hit and solved, restated as checks. Waku started on gpui-component and later replaced it in the transcript and markdown; the gpui-kit items are checked against 0.6.6 in [Verification](#verification-gpui-kit-066). A failing item is fixed upstream where possible, otherwise covered here.

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

## Verification (gpui-kit 0.6.6)

Our tests are in [tests/gpui_kit.rs](tests/gpui_kit.rs); other evidence is gpui-kit's or GPUI's own tests and source. Paths are relative to each crate's `src/`.

| Check | Result | Evidence | Route |
|---|---|---|---|
| Unmeasured rows are unknown | Pass in GPUI; gpui-kit's `is_scrolled_up` counts unknown as scrolled up, so the jump button can show for a frame while following is off | gpui-pre `list.rs` `test_follow_tail_reengagement_not_fooled_by_unmeasured_items`; gpui-component `message_scroller.rs:58-63` | one-line upstream fix |
| Prompt pinned at top while the reply grows | Missing: `MessageScroller` fixes alignment and follow mode and has no end space | gpui-component `message_scroller.rs:36-38` | trailing spacer row sized to the viewport, or an upstream option |
| User scroll, scroll back, scrollbar drag | Pass | gpui-pre `list.rs` `test_follow_tail_disengages_on_user_scroll`, `…_on_scrollbar_reposition`, `…_reengages_when_scrolled_back_to_bottom`, `…_reengages_after_scrollbar_drag_to_bottom_while_growing` | use as is |
| Remeasure only changed rows | Pass, when the caller remeasures the changed row | `remeasure_items`, `splice`; gpui-pre `test_remeasure_item_preserves_scroll_offset` | caller calls `remeasure_items` |
| Expanding a row while following keeps it under the pointer | Missing: following snaps back to the tail; GPUI's `ListState::pause_following_tail` would hold it, but `MessageScrollerState` does not expose it | gpui-pre `elements/list.rs:646`; gpui-component `message_scroller.rs:26` | one-method upstream addition |
| Unclosed markers hidden while streaming | Fail: `Hello **bol` renders `**` | `an_unclosed_emphasis_does_not_show_its_marker_while_streaming` (ignored) | upstream, or accept the literal markers: mending in the app makes the text non-append, which loses the fade |
| Streamed parse equals full parse | Pass for tables, lists, fences | `a_streamed_table_renders_like_a_full_parse`, `a_streamed_list_and_fence_render_like_a_full_parse`; last block reparsed, gpui-base `text/state.rs:961` | use as is |
| Fade is paint-only, refades past the common prefix, full opacity on attach | Pass | gpui-base `text/inline.rs:103`, `text/stream_fade.rs` `record`, `note_replace` | use as is |
| Fade, `Spinner`, `ShimmerText` redraw rate | Fail: display rate while visible; the fade costs 3.5 s CPU per 17 s reply | measured on the spike; gpui-base `text/state.rs:761` | upstream (issue drafted); local patch meanwhile |
| IME with multibyte text before the caret | Pass | gpui-base `input/base/state.rs` `test_ime_selection_is_relative_to_replacement_start` | use as is |
| Undo per gesture, composition as one step | Pass | `test_ime_composition_undoes_as_one_unit`, `test_undo_manager_composition_cancel_leaves_no_entry`, `test_edit_after_composition_is_separate_undo` | use as is |
| Enter family and modified deletes | Pass: `shift-`, `ctrl-`, `alt-backspace` bound; Enter submits or inserts by `submit_on_enter` | gpui-base `input/base/state.rs:133-150`, `:1887` | use as is |
| Paste images and files | Hook exists; ordering is ours | gpui-component `input/textarea.rs:123` `on_paste` | composer |
| An opened overlay handles its keys | Pass: Escape closes it once opened | `an_opened_popover_takes_focus_so_escape_closes_it` | use as is |
| Trigger click closes an open popover | Pass | `clicking_the_trigger_of_an_open_popover_closes_it` | use as is |
| Selection across views and blocks, copy in document order | Pass: a window-level selection layer joins text views in document order and caches their copies | gpui-base `text_selection.rs` `plain_projection_caches_multiple_participant_copies_in_document_order`, `cross_participant_selection_excludes_participants_outside_its_document_interval`, `shift_extension_falls_back_when_the_anchor_participant_was_swept` | wire the transcript into one selection scope |
| Selection when its start scrolls out of view; soft-wrap row starts | Not tested; gpui-base falls back when the anchor view is swept, where Waku keeps the spans | | check by hand |
| Enter while an IME composes | Not tested (OS input path) | | check by hand with Pinyin |
| Streaming code highlight stability | Not tested; highlighting needs the `tree-sitter` feature | | decide when code blocks are styled |

Nothing found forces the transcript or markdown off gpui-kit. The gaps are the fade rate, marker mending, prompt pinning and pausing the follow while a row expands, each fixable upstream or in the list's row content.
