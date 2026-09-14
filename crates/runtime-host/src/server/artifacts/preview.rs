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

use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_event_log::artifacts::ArtifactChunk;
use maka_protocol::artifact::*;
use maka_runtime::artifact::Artifact;

pub(super) fn text(chunk: Option<ArtifactChunk>) -> Result<TextPreview, ReadFailure> {
    let chunk = chunk.ok_or(ReadFailure::NotFound)?;
    if chunk.total_bytes > MAX_PREVIEW_BYTES as u64 {
        return Err(ReadFailure::TooLarge);
    }
    let text = String::from_utf8_lossy(&chunk.bytes).into_owned();
    if text.len() > MAX_PREVIEW_BYTES {
        return Err(ReadFailure::TooLarge);
    }
    Ok(TextPreview { text })
}

pub(super) fn binary(chunk: Option<ArtifactChunk>) -> Result<BinaryPreview, BinaryReadFailure> {
    let chunk = chunk.ok_or(BinaryReadFailure::NotFound)?;
    if chunk.total_bytes > MAX_PREVIEW_BYTES as u64 {
        return Err(BinaryReadFailure::TooLarge);
    }
    let mime_type = sniff(&chunk.bytes)
        .ok_or(BinaryReadFailure::UnsupportedMime)?
        .into();
    Ok(BinaryPreview {
        base64: STANDARD.encode(chunk.bytes),
        mime_type,
    })
}

pub(super) fn project(mut artifact: Artifact) -> Artifact {
    artifact.name = project_text(&artifact.name, 512);
    artifact.mime_type = artifact.mime_type.map(|text| project_text(&text, 512));
    artifact.summary = artifact.summary.map(|text| project_text(&text, 8 * 1024));
    artifact
}

fn project_text(text: &str, max: usize) -> String {
    let mut result = String::new();
    for character in text.chars() {
        let character = if character <= '\u{1f}' || character == '\u{7f}' {
            '\u{fffd}'
        } else {
            character
        };
        if result.len() + character.len_utf8() > max {
            break;
        }
        result.push(character);
    }
    if result.is_empty() {
        "artifact".into()
    } else {
        result
    }
}

use maka_runtime::attachment::sniff_binary_mime as sniff;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_bound_decoded_text_and_sniff_payload_instead_of_declared_mime() {
        let chunk = |bytes: Vec<u8>| {
            Some(ArtifactChunk {
                total_bytes: bytes.len() as u64,
                bytes,
            })
        };
        assert_eq!(text(chunk(vec![0xff; 12_000])), Err(ReadFailure::TooLarge));
        assert_eq!(
            text(chunk(vec![0xff])),
            Ok(TextPreview {
                text: "\u{fffd}".into()
            })
        );
        for (bytes, expected) in [
            (b"not an image".as_slice(), None),
            (b"\x89PNG\r\n\x1a\n", Some("image/png")),
            (b"preamble %PDF-1.7", Some("application/pdf")),
            (
                b"\xef\xbb\xbf <?xml?><SVG viewBox='0'/>",
                Some("image/svg+xml"),
            ),
            (b"<svg/>", None),
            (b"<svgx>", None),
        ] {
            assert_eq!(sniff(bytes), expected);
            let result = binary(chunk(bytes.to_vec()));
            assert_eq!(result.as_ref().ok().map(|p| p.mime_type.as_str()), expected);
        }
    }
}
