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

use super::{MAX_SUMMARY_BYTES, SummaryDefect};
use crate::model::ModelUsage;

pub const SUMMARY_FORMAT_TEMPLATE: &str = "## Goal\n[What the user is trying to accomplish]\n\n## Progress\n### Done\n- [Completed work and changes]\n### In Progress\n- [Current work]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Files, commands/results, errors, anything needed to continue; or \"(none)\"]";
const REQUIRED: [&str; 4] = [
    "## Goal",
    "## Progress",
    "## Next Steps",
    "## Critical Context",
];

/// Port of history-compact-summary-validation.ts. The compaction layer's empty
/// gate is included here; supplied usage must belong to the initial fold only.
pub fn validate_summary(
    text: &str,
    initial_usage: Option<&ModelUsage>,
) -> Result<(), SummaryDefect> {
    if text.len() > MAX_SUMMARY_BYTES {
        return Err(SummaryDefect::TooLarge);
    }
    let text = text.trim_matches(js_space);
    if text.is_empty() {
        return Err(SummaryDefect::Empty);
    }
    let (sections, open_fence) = scan(text);
    if !sections {
        return Err(SummaryDefect::MissingSection);
    }
    if open_fence
        || text.ends_with("...")
        || text.ends_with([':', '：', ',', '，', '、', ';', '；', '…', '(', '（', '—'])
    {
        return Err(SummaryDefect::Truncated);
    }
    if initial_usage.is_some_and(|usage| matches!((usage.input_tokens, usage.output_tokens), (Some(input), Some(output)) if input > 10_000 && output < 200)) {
        return Err(SummaryDefect::TooSmallForFold);
    }
    Ok(())
}

fn scan(text: &str) -> (bool, bool) {
    let mut open: Option<(u8, usize)> = None;
    let mut matched = 0;
    let mut attributes = false;
    let mut content = [false; 4];
    for line in text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
    {
        let count = |content: &mut [bool; 4]| {
            let trimmed = line.trim_matches(js_space);
            if matched > 0
                && attributes
                && !trimmed.is_empty()
                && !SUMMARY_FORMAT_TEMPLATE.lines().any(|line| line == trimmed)
                && !bare_fence(trimmed)
            {
                content[matched - 1] = true;
            }
        };
        let structural = markdown_start(line);
        if let Some((family, width)) = structural.and_then(fence_run) {
            if open.is_none() {
                open = Some((family, width));
            } else if open
                .is_some_and(|(old_family, old_width)| family == old_family && width >= old_width)
                && line.trim_matches(js_space).len() == width
            {
                open = None;
            } else {
                count(&mut content);
            }
            continue;
        }
        if open.is_some() {
            count(&mut content);
            continue;
        }
        if matched < REQUIRED.len()
            && structural.is_some_and(|line| line.starts_with('#'))
            && line.trim_matches(js_space) == REQUIRED[matched]
        {
            matched += 1;
            attributes = true;
            continue;
        }
        let heading = structural.and_then(heading_level);
        if heading == Some(2) {
            attributes = false;
            continue;
        }
        if heading.is_none() && !thematic_break(line) {
            count(&mut content);
        }
    }
    (
        matched == REQUIRED.len() && content.into_iter().all(|has| has),
        open.is_some(),
    )
}

// Four spaces make indented code, not a structural Markdown delimiter.
fn markdown_start(line: &str) -> Option<&str> {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    (spaces <= 3).then(|| &line[spaces..])
}
fn fence_run(line: &str) -> Option<(u8, usize)> {
    let family = *line.as_bytes().first()?;
    if !matches!(family, b'`' | b'~') {
        return None;
    }
    let width = line.bytes().take_while(|byte| *byte == family).count();
    (width >= 3).then_some((family, width))
}
fn bare_fence(line: &str) -> bool {
    fence_run(line).is_some_and(|(_, width)| width == line.len())
}
fn heading_level(line: &str) -> Option<usize> {
    let width = line.bytes().take_while(|byte| *byte == b'#').count();
    ((1..=6).contains(&width) && line[width..].chars().next().is_none_or(js_space)).then_some(width)
}
fn thematic_break(line: &str) -> bool {
    let mut chars = line.chars().filter(|c| !js_space(*c));
    let Some(first) = chars.next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 1;
    for character in chars {
        if character != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}
pub(super) fn js_space(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
