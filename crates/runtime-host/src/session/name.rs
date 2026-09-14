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

use maka_protocol::{ProtocolError, Result};
use unicode_normalization::UnicodeNormalization;

/// Matches core/text-sanitize's NFC, control/bidi, invisible and code-point policy.
pub(super) fn normalize(input: &str) -> Result<String> {
    let cleaned: String = input.nfc().filter_map(|ch| {
        let code = ch as u32;
        if matches!(code, 0x200b..=0x200d | 0x2060..=0x2064 | 0xfeff) {
            None
        } else if matches!(code, 0..=0x1f | 0x7f..=0x9f | 0x61c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x206f) {
            Some(' ')
        } else {
            Some(ch)
        }
    }).collect();
    let name: String = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    if name.is_empty() {
        return Err(ProtocolError::invalid(
            "Session name cannot be empty after sanitization",
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn canonical_names_remove_display_controls_without_splitting_unicode() {
        for (input, expected) in [
            ("  cafe\u{301}\n\u{202e}two\u{200b}  ", "café two"),
            ("a\u{206f}b\u{200d}c", "a bc"),
        ] {
            assert_eq!(normalize(input).unwrap(), expected);
        }
        let capped = normalize(&"🦊".repeat(81)).unwrap();
        assert_eq!(capped.chars().count(), 80);
        assert!(normalize("\u{200b}\u{202e} \n").is_err());
    }
}
