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

//! Closes what a streaming reply has opened but not yet closed, in a copy
//! used only for display, so `**bo` shows as bold "bo" rather than as `**bo`.
//! Only the last paragraph is touched and only by trimming or appending at
//! its end, so every earlier byte keeps its source offset. Code is never
//! mended: an open fence already renders as code.

use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq)]
enum Open {
    Strong,
    Emphasis,
    Strike,
}

impl Open {
    fn marker(self) -> &'static str {
        match self {
            Self::Strong => "**",
            Self::Emphasis => "*",
            Self::Strike => "~~",
        }
    }
}

pub fn mend(source: &str) -> Cow<'_, str> {
    if in_open_fence(source) {
        return source.into();
    }
    let start = source.rfind("\n\n").map_or(0, |at| at + 2);
    let paragraph = &source[start..];
    let bytes = paragraph.as_bytes();
    let mut stack: Vec<(Open, usize)> = Vec::new();
    let mut code: Option<(usize, usize)> = None;
    let mut url = false;
    let mut partial = None;
    let mut ix = 0;
    while ix < bytes.len() {
        let byte = bytes[ix];
        if byte == b'\\' {
            ix += 2;
            continue;
        }
        let run = bytes[ix..].iter().take_while(|next| **next == byte).count();
        if byte == b'`' {
            code = match code {
                Some((ticks, _)) if ticks == run => None,
                Some(open) => Some(open),
                None => Some((run, ix)),
            };
            ix += run;
            continue;
        }
        if code.is_some() {
            ix += run;
            continue;
        }
        match byte {
            b'*' | b'~' => {
                let line_start = ix == 0 || bytes[ix - 1] == b'\n';
                let next = bytes.get(ix + run).copied();
                let before = ix.checked_sub(1).map(|at| bytes[at]);
                // A list bullet, or a lone marker between spaces, is not emphasis.
                let bullet = byte == b'*' && run == 1 && line_start && next == Some(b' ');
                let can_open = next.is_some_and(|next| !next.is_ascii_whitespace());
                let can_close = before.is_some_and(|before| !before.is_ascii_whitespace());
                let wanted: &[Open] = match (byte, run) {
                    (b'*', 1) => &[Open::Emphasis],
                    (b'*', 2) => &[Open::Strong],
                    (b'*', 3) => &[Open::Strong, Open::Emphasis],
                    (b'~', 2) => &[Open::Strike],
                    _ => &[],
                };
                // Half of a `~~` still arriving.
                if byte == b'~' && run == 1 && next.is_none() {
                    partial = Some(ix);
                }
                if !bullet {
                    for open in wanted {
                        match stack.iter().rposition(|(kind, _)| kind == open) {
                            Some(at) if can_close => {
                                stack.truncate(at);
                            }
                            _ if can_open || next.is_none() => stack.push((*open, ix)),
                            _ => {}
                        }
                    }
                }
                ix += run;
            }
            b']' if bytes.get(ix + 1) == Some(&b'(') => {
                url = true;
                ix += 2;
            }
            b')' if url => {
                url = false;
                ix += 1;
            }
            _ => ix += run,
        }
    }
    if stack.is_empty() && code.is_none() && !url && partial.is_none() {
        return source.into();
    }
    let mut mended = String::with_capacity(source.len() + 8);
    // A marker with nothing after it yet is dropped rather than closed.
    let cut = stack
        .iter()
        .chain(code.map(|(_, at)| (Open::Strong, at)).as_ref())
        .filter(|(_, at)| {
            paragraph[*at..]
                .trim_start_matches(['*', '~', '`'])
                .trim()
                .is_empty()
        })
        .map(|(_, at)| *at)
        .chain(partial)
        .min()
        .unwrap_or(paragraph.len());
    mended.push_str(source[..start + cut].trim_end());
    if let Some((ticks, at)) = code
        && at < cut
    {
        mended.push_str(&"`".repeat(ticks));
    }
    if url && code.is_none() {
        mended.push(')');
    }
    for (open, at) in stack.iter().rev() {
        if *at < cut {
            mended.push_str(open.marker());
        }
    }
    mended.into()
}

fn in_open_fence(source: &str) -> bool {
    let mut fence: Option<(u8, usize)> = None;
    for line in source.lines() {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let Some(&first) = trimmed.as_bytes().first() else {
            continue;
        };
        if first != b'`' && first != b'~' {
            continue;
        }
        let run = trimmed.bytes().take_while(|byte| *byte == first).count();
        if run < 3 {
            continue;
        }
        fence = match fence {
            None => Some((first, run)),
            Some((open, len))
                if open == first && run >= len && trimmed[run..].trim().is_empty() =>
            {
                None
            }
            open => open,
        };
    }
    fence.is_some()
}

#[cfg(test)]
mod tests {
    use super::mend;
    use crate::md::parse::parse;

    fn rendered(source: &str) -> String {
        parse(source, 0)
            .blocks
            .iter()
            .flat_map(|block| block.texts.iter().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn closes_what_the_tail_left_open() {
        assert_eq!(mend("Hello **bol"), "Hello **bol**");
        assert_eq!(mend("a *b **c"), "a *b **c***");
        assert_eq!(mend("run `cargo te"), "run `cargo te`");
        assert_eq!(
            mend("see [docs](https://x.y/pa"),
            "see [docs](https://x.y/pa)"
        );
        assert_eq!(mend("gone ~~old"), "gone ~~old~~");
    }

    #[test]
    fn drops_a_marker_with_nothing_after_it() {
        assert_eq!(mend("Hello **"), "Hello");
        assert_eq!(mend("Hello `"), "Hello");
        assert_eq!(mend("Hello **bold** and *"), "Hello **bold** and");
    }

    #[test]
    fn leaves_code_bullets_and_closed_text_alone() {
        assert_eq!(mend("```rust\nlet a = **b"), "```rust\nlet a = **b");
        assert_eq!(mend("* item"), "* item");
        assert_eq!(mend("2 * 3 = 6"), "2 * 3 = 6");
        assert_eq!(mend("done **now**."), "done **now**.");
        assert_eq!(mend("`a*b`"), "`a*b`");
        assert_eq!(mend("**old**\n\nnew"), "**old**\n\nnew");
    }

    #[test]
    fn mending_twice_changes_nothing() {
        for source in ["Hello **bol", "a *b **c", "run `x", "[a](b", "x ~~y"] {
            let once = mend(source).into_owned();
            assert_eq!(mend(&once), once, "{source}");
        }
    }

    #[test]
    fn no_prefix_of_a_reply_shows_a_marker() {
        let reply = "Use **cargo test** to run `the suite`, then *read* the ~~old~~ [notes](https://n.example).";
        for end in (1..=reply.len()).filter(|end| reply.is_char_boundary(*end)) {
            let shown = rendered(&mend(&reply[..end]));
            assert!(
                !shown.contains(['*', '`', '~']) && !shown.contains("]("),
                "{:?} shows {shown:?}",
                &reply[..end]
            );
        }
    }
}
