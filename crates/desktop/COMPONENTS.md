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

The component inventory and acceptance standard for the GPUI client. Visual values come from the root [DESIGN.md](../../DESIGN.md); this file decides which components exist, what gpui-kit already provides, and what each component must prove before it ships.

Scope is the committed product: chat, sessions and settings over `maka-client`. Surfaces the Host does not serve (goal, plan, memory, usage, agent graph) get no component.

## Layers

| Layer | Owner | Rule |
|---|---|---|
| Behavior: input, IME, selection, focus, scrolling, overlays, markdown | gpui-kit (`gpui-base`, `gpui-component`) | Use as is. Defects are patched locally and sent upstream, never forked. |
| Tokens: surfaces, ink, borders, radius, spacing, type, motion | `src/ui/theme.rs` | One Rust mapping of DESIGN.md. Values that gpui-kit's `ThemeColor` has no slot for live in our own `Global`. |
| Product components | `src/ui/` | Plain-data inputs only. No `maka_protocol` type crosses a component boundary; the chat and settings views translate protocol rows into component props. |

## Tokens

- **Surface ladder** sunken / base / raised / overlay and **borders** soft / default / strong: our `Global`, because `ThemeColor` cannot be extended. `ThemeColor` fields are filled from the same values so kit components agree with ours.
- **Ink**: two tiers, `foreground` and `muted_foreground`. No third grey.
- **Radius**: 6 control, 10 surface, 12 modal, 28 chat (user bubble and composer), pill. Set gpui-kit `radius` / `radius_lg` to 10 / 12.
- **Controls**: 20 / 24 / 28 / 32 / 36 / 40 px ruler, 32 default. Icon buttons are 28.
- **Hover**: one wash token for rows and controls. Hover and press are instant; no color interpolation.
- **Reading measure**: one token for the transcript column and the composer. Components never set their own max width.
- **Status vocabulary**: success, active, attention, error, neutral. Every status dot resolves through it.
- **Motion**: durations and easings from gpui-kit `MotionTokens`; every animation honours `cx.reduce_motion()`.

## Inventory

Priority: **P0** is needed for the chat to be usable day to day, **P1** completes the committed scope, **P2** can wait.

### Shell and sessions

| Component | Content | gpui-kit base | Our work | P |
|---|---|---|---|---|
| Window shell | transparent titlebar, sidebar + main split | `TitleBar`, `h_resizable` | composition; per-region vibrancy is not available (macOS blur is whole-window only) | P0 |
| Session row | name, one muted line (preview or workspace, relative time), status dot (running, waiting, blocked, unread), hover actions, context menu | `Sidebar` shell only; `SidebarMenuItem` has no body slot | own `SidebarItem` | P0 |
| Session groups | by recency or project | `SidebarGroup` | grouping rules | P1 |
| Rename / remove dialogs | | `Dialog`, `AlertDialog` | copy | P1 |

### Transcript

| Component | Content | gpui-kit base | Our work | P |
|---|---|---|---|---|
| Transcript column | tail-following list, jump to latest | `MessageScroller` | measure container, turn spacing | P0 |
| User message | text, reference and attachment chips | `Bubble` | chips; 28 radius | P0 |
| Assistant message | markdown, no bubble, no avatar; hover footer with copy and time | `TextView`, `Clipboard` | hover footer | P0 |
| Reasoning block | collapsed "Thinking" summary, streams while open | `Collapsible` | summary line, live state | P0 |
| Tool row | icon, verb, target, status, duration; expands to arguments and output | `Collapsible` | the row and its nine states: pending, waiting, running, returned, attention, failed, timed out, cancelled, missing | P0 |
| Turn status | working dots with elapsed time, provider retry, compacting, failed or aborted notice with resume | none suitable (see Motion) | own | P0 |
| Interaction card: permissions | reason, command in a mono well, deny + Once / Turn / Session | `Button` | card on the floating recipe, pinned above the composer | P0 |
| Composer | card with borderless textarea, toolbar row, round send / stop | `Textarea` (`appearance(false)`, `auto_grow`) | card, toolbar, send state | P0 |
| Code block header | language label, copy | `code_block_actions` overlay | decide overlay vs. own block via `markdown_block_parser` (loses built-in highlighting) | P1 |
| Tool group | consecutive reads and searches folded to one line; finished turn folds to "Worked for Ns" | `Collapsible` | grouping rules | P1 |
| Diff view | file header with +/− counts, line-level backgrounds | none (`Editor` decorations color ranges, not lines) | own | P1 |
| Shell output | mono output, exit code, truncation with expand | none | own; ANSI later | P1 |
| Interaction card: question, form, capability | options, form fields, allow / deny | `Form`, `Radio`, `Checkbox`, `Input` | card variants | P1 |
| Queue | steering and follow-up items with edit, retract | `List` | rows; needs client wrappers for queue ops | P1 |
| Model chip | model and thinking level per session | `Combobox` / `Popover` | trigger chip | P1 |
| Empty states | three tiers from DESIGN.md §10 | `Empty` | tier presets | P1 |
| Toast | | `Notification` | | P1 |
| Context meter | | `ProgressCircle` | | P2 |
| Composer attachments | | `Attachment` | upload flow | P2 |

### Settings

| Component | Content | gpui-kit base | Our work | P |
|---|---|---|---|---|
| Settings shell | pages with search | `Settings` / `SettingPage` / `SettingItem` | page layout | P1 |
| Connection list and detail | status, last test, fetch, set default, remove | `List`, `Button`, `Badge` | rows | P1 |
| Model inventory | enable toggles, overrides | `List`, `Switch` | rows | P1 |
| Onboarding form | provider, configuration, model selection | `Form`, `Select`, `Input` | form-level error line (kit validates per input only) | P1 |
| Credential field | masked secret, status | `Input` | | P1 |
| OAuth login | URL and code presentation | `Dialog` | | P2 |

### Shared primitives

- **Icon button** (28, ghost) and **status dot**: thin wrappers so size and color come from tokens, not call sites.
- **Hover reveal**: GPUI `group` / `group_hover`, one pattern for every hover-only action.
- **Motion clock**: one app-wide low-frequency clock (~30 fps) that animated views lease while they animate and that stops when no lease remains. gpui-kit's `Spinner`, `ShimmerText` and stream fade redraw through `request_animation_frame`, which holds the window at display rate (120 Hz on ProMotion) for as long as they are visible. Every loader and the stream fade go through this clock; the stream-fade fix is also sent upstream.

## Acceptance standard

Every component, before it is used in a view:

1. **Spec** (a short section in the PR, not this file): purpose, anatomy, sizes from the ruler, tokens used, keyboard behaviour, copy.
2. **States** shown in the gallery, light and dark: default, hover, pressed, selected, focus-visible, disabled, loading, error, and each domain state (the nine tool states, the five status meanings). A state that cannot occur is listed as such.
3. **Inputs** are plain data; a view test builds the component without a Host.
4. **Motion** declares duration and easing, runs on the motion clock, and is still with reduce motion.
5. **Idle cost**: zero redraws when nothing animates; streaming stays under 15% of one core in the release build.
6. **Layout**: loading and ready occupy the same box (no shift); text lines sit on the 4 px grid.

The gallery is a second window (`maka-desktop --gallery`) that renders every component in every state from fixtures. Visual review happens there, not in a live session.

## Order

1. Tokens, motion clock, gallery window.
2. P0 components, then replace the spike's ad-hoc chat and sidebar rendering with them.
3. P1 by surface: transcript, then settings.
