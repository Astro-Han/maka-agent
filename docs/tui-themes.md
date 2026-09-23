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

# Rust TUI themes

Open **Settings → Customize theme**. Select a color role, then click a preset
swatch or enter `#RRGGBB`. Maka, Dusk and Paper are starting palettes.
The preview updates immediately; editor controls keep their original colors
so they remain usable even while you experiment with low-contrast colors.

- Mouse: click a role, swatch or text field; scroll the role list.
- Keyboard: Tab/Shift+Tab moves between name, starting palette, role list,
  swatches, hex input and footer actions. Arrow keys navigate the focused
  list/palette. Enter applies the selected swatch; Ctrl+A replaces a text field.
- **Save & apply** writes the displayed file and selects the custom theme.
- **Cancel**, Escape or an outside click discards an unsaved preview.
- **Load custom theme** reads edits made in another editor into the preview.
  It does not write the file. Once Save is pressed, hiding the dialog does not
  cancel the file write.

The first Settings row cycles Maka → Dusk → Paper → Terminal → custom
(if loaded) → Maka. Theme selection is local to the TUI Root/profile, not a
Host model setting. The theme definition file is shared by profiles using the
same path. Terminal uses the terminal's ANSI palette; custom themes use RGB.

## File location

The editor displays its destination. Default locations:

| Platform | File |
| --- | --- |
| Linux / WSL | `~/.local/share/Maka/tui/theme.json` |
| macOS | `~/Library/Application Support/Maka/tui/theme.json` |
| Windows | `%USERPROFILE%/AppData/Local/Maka/tui/theme.json` |

If `MAKA_TUI_STATE_DIR` is set, the default is `theme.json` inside that directory.
To select a different definition, set `MAKA_TUI_THEME` to an **absolute path**:

```sh
MAKA_TUI_THEME=/absolute/path/aurora.json maka
```

Files are read on startup and explicitly reloaded, not watched. A missing
default file is normal until the first Save. Invalid/unreadable files do not
prevent startup: the last successfully loaded colors remain in memory, or Maka
is used when a saved custom choice has no valid definition.

## Editable format

A minimal definition inherits omitted colors from `base`:

```json
{
  "version": 1,
  "name": "Aurora",
  "base": "maka",
  "colors": {
    "accent": "#71CCD1",
    "thinking": "#BE9DF7"
  },
  "syntax": {
    "keyword": "#BE9DF7",
    "string": "#8ECCA7"
  }
}
```

`base` may be `maka`, `dusk` or `paper`; omitted means `maka`.
Names must be nonempty, at most 40 characters, and contain no control or bidi
override/isolate characters. Colors must use six hex digits; alpha and named
colors are not accepted.

`colors` supports: `background`, `foreground`, `surface`, `accent`,
`thinking`, `success`, `warning`, `error`, `muted`, `subtle`, `border`,
`scrollbar`, `selection`, `selection-text`, `search`, `search-active`,
`search-text`.

`syntax` supports: `keyword`, `string`, `comment`, `number`, `function`,
`type`, `operator`.

The visual editor writes a complete resolved palette. Unknown/duplicate fields,
unsupported versions and files larger than 16 KiB are rejected. Definitions
must be regular files, not symlinks or devices; no scripts or external assets
are loaded. Error feedback stays in Settings/the theme editor.

Saving uses a temporary file and atomic replacement. Before replacement it
checks that the destination still matches the last loaded bytes (or remains
absent for a new file); detected external edits are preserved and require
reloading. This is optimistic conflict detection, not a filesystem transaction
against arbitrary external writers. Custom color contrast remains your choice;
the shipped RGB themes have automated readability checks.
