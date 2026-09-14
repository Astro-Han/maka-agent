// Portions adapted from OpenAI Codex under the Apache License 2.0.
// Copyright 2025 OpenAI. See SOURCE.md and the repository LICENSE/NOTICE.
// Maka modifications: pure bounded APIs, no filesystem/execution integration,
// restricted patch grammar and explicit preparation limits. SOURCE.md records
// the exact upstream files/revisions and file-specific changes.

//! Pure, bounded patch preparation. Paths carry no filesystem authority.
mod parser;
mod seek_sequence;
mod text_file;
mod update;

use std::path::PathBuf;
use thiserror::Error;

/// Maximum bytes in a patch, source file, or prepared result.
pub const MAX_BYTES: usize = 1024 * 1024;
const MAX_LINES: usize = 32_768;
const MAX_OPERATIONS: usize = 128;
const MAX_MATCH_WORK: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum PatchError {
    #[error("invalid patch: {0}")]
    InvalidPatchError(String),
    #[error("invalid hunk at line {line_number}: {message}")]
    InvalidHunkError { message: String, line_number: usize },
    #[error("{0}")]
    ComputeReplacements(String),
    #[error("patch preparation exceeds {0} limit")]
    Limit(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatchOperation {
    Add {
        path: PathBuf,
        content: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        chunks: Vec<UpdateChunk>,
    },
}

/// Parsed by this crate; opaque fields keep replacement indices trustworthy.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    context_line_indices: Vec<(usize, usize)>,
    is_end_of_file: bool,
}

impl UpdateChunk {
    fn push_context_line(&mut self, line: String) {
        self.context_line_indices
            .push((self.old_lines.len(), self.new_lines.len()));
        self.old_lines.push(line.clone());
        self.new_lines.push(line);
    }
}

fn check_input(text: &str) -> Result<(), PatchError> {
    if text.len() > MAX_BYTES {
        return Err(PatchError::Limit("bytes"));
    }
    // SourceFile recognizes lone CR as well as LF/CRLF. Count the same
    // logical lines without allocating the parsed source before admission.
    let lines = text
        .bytes()
        .enumerate()
        .filter(|(index, byte)| {
            *byte == b'\n' || (*byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n'))
        })
        .count()
        + usize::from(!text.is_empty() && !text.ends_with(['\r', '\n']));
    if lines > MAX_LINES {
        return Err(PatchError::Limit("lines"));
    }
    Ok(())
}

/// Parse the client-executed Codex envelope. Move, shell wrappers, environment
/// directives and empty operations are unsupported. Operations retain source
/// order; callers must independently authorize and settle each mutation.
pub fn parse_patch(patch: &str) -> Result<Vec<PatchOperation>, PatchError> {
    check_input(patch)?;
    let lines: Vec<_> = patch.trim().lines().collect();
    if lines.len() < 2
        || lines[0].trim() != "*** Begin Patch"
        || lines.last().map(|line| line.trim()) != Some("*** End Patch")
    {
        return Err(PatchError::InvalidPatchError(
            "expected Begin Patch / End Patch envelope".into(),
        ));
    }
    let mut remaining = &lines[1..lines.len() - 1];
    let mut line_number = 2;
    let mut operations = Vec::new();
    while !remaining.is_empty() {
        if operations.len() >= MAX_OPERATIONS {
            return Err(PatchError::Limit("operations"));
        }
        let (operation, consumed) = parser::parse_one_hunk(remaining, line_number)?;
        let path = match &operation {
            PatchOperation::Add { path, content } => {
                if content.is_empty() {
                    return Err(PatchError::InvalidPatchError("Add has no diff".into()));
                }
                path
            }
            PatchOperation::Delete { path } | PatchOperation::Update { path, .. } => path,
        };
        if path.as_os_str().is_empty() || path.to_string_lossy().contains('\0') {
            return Err(PatchError::InvalidPatchError(
                "invalid empty or NUL path".into(),
            ));
        }
        operations.push(operation);
        line_number += consumed;
        remaining = &remaining[consumed..];
    }
    if operations.is_empty() {
        return Err(PatchError::InvalidPatchError(
            "patch has no operations".into(),
        ));
    }
    Ok(operations)
}

/// Parse a structured OpenAI update diff using a fixed virtual path. A diff
/// cannot inject another operation or replace the virtual target.
pub fn parse_update(diff: &str) -> Result<Vec<UpdateChunk>, PatchError> {
    check_input(diff)?;
    let diff = diff
        .strip_suffix("\r\n")
        .or_else(|| diff.strip_suffix('\n'))
        .unwrap_or(diff);
    let patch =
        format!("*** Begin Patch\n*** Update File: maka-virtual-update\n{diff}\n*** End Patch");
    let operations = parse_patch(&patch)?;
    match operations.as_slice() {
        [PatchOperation::Update { path, chunks }]
            if path == &PathBuf::from("maka-virtual-update") =>
        {
            Ok(chunks.clone())
        }
        _ => Err(PatchError::InvalidPatchError(
            "expected exactly one update diff".into(),
        )),
    }
}

/// Parse a structured OpenAI create diff. Unlike an envelope Add, this joins
/// added lines without forcing a final newline (the SDK's native contract).
pub fn parse_create(diff: &str) -> Result<String, PatchError> {
    check_input(diff)?;
    let patch = format!(
        "*** Begin Patch\n*** Add File: maka-virtual-create\n{}\n*** End Patch",
        diff.strip_suffix("\r\n")
            .or_else(|| diff.strip_suffix('\n'))
            .unwrap_or(diff)
    );
    let operations = parse_patch(&patch)?;
    match operations.as_slice() {
        [PatchOperation::Add { path, content }]
            if path == &PathBuf::from("maka-virtual-create") =>
        {
            let mut content = content.clone();
            content.pop(); // Remove only the newline inserted by upstream Add parsing.
            Ok(content)
        }
        _ => Err(PatchError::InvalidPatchError(
            "expected exactly one create diff".into(),
        )),
    }
}

/// Apply fuzzy chunks to a snapshot, preserving original context and mixed
/// line endings. Insertions use the first source line ending (LF if absent).
/// Like upstream, updates give an unterminated final line a line ending.
/// Matching is not linear: conservative work limits bound adversarial inputs.
pub fn apply_update(source: &str, chunks: &[UpdateChunk]) -> Result<String, PatchError> {
    check_input(source)?;
    if chunks.is_empty() {
        return Err(PatchError::InvalidPatchError("empty update".into()));
    }
    if chunks.len() > MAX_LINES {
        return Err(PatchError::Limit("chunks"));
    }
    let mut source_file = text_file::SourceFile::parse(source);
    let lines = source_file.line_texts();
    let mut pattern_bytes = 0usize;
    let mut pattern_lines = 0usize;
    let mut inserted_bytes = 0usize;
    for chunk in chunks {
        pattern_lines = pattern_lines
            .saturating_add(chunk.old_lines.len() + usize::from(chunk.change_context.is_some()));
        pattern_bytes =
            pattern_bytes.saturating_add(chunk.change_context.as_ref().map_or(0, |s| s.len() + 1));
        for line in &chunk.old_lines {
            pattern_bytes = pattern_bytes.saturating_add(line.len() + 1);
        }
        for line in &chunk.new_lines {
            inserted_bytes = inserted_bytes.saturating_add(line.len() + 2);
        }
    }
    let work = lines
        .len()
        .saturating_mul(pattern_bytes)
        .saturating_add(source.len().saturating_mul(pattern_lines));
    if work > MAX_MATCH_WORK {
        return Err(PatchError::Limit("matching work"));
    }
    if inserted_bytes > MAX_BYTES {
        return Err(PatchError::Limit("inserted bytes"));
    }
    let replacements = update::compute_replacements(&lines, "snapshot", chunks)?;
    let mut previous_end = 0usize;
    for (start, length, _) in &replacements {
        if *start < previous_end || start.saturating_add(*length) > lines.len() {
            return Err(PatchError::ComputeReplacements(
                "overlapping replacements".into(),
            ));
        }
        previous_end = start + length;
    }
    source_file.apply_replacements(&replacements);
    let result = source_file.into_contents();
    if result.len() > MAX_BYTES {
        return Err(PatchError::Limit("output bytes"));
    }
    Ok(result)
}
