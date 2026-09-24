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
