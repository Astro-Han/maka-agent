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

use super::Image;
use gix::diff::blob::{
    self,
    unified_diff::{ConsumeHunk, DiffLineKind, HunkHeader},
};
use std::io::{self, Write};

mod binary;

pub(super) fn append(
    out: &mut Vec<u8>,
    name: &[u8],
    before: Option<&Image>,
    after: Option<&Image>,
    hash: gix::hash::Kind,
) -> io::Result<()> {
    if before.map(|i| (i.mode, &i.bytes)) == after.map(|i| (i.mode, &i.bytes)) {
        return Ok(());
    }
    // A file-kind replacement is represented as delete + add, not an invalid mode transition.
    if let (Some(old), Some(new)) = (before, after)
        && (old.mode & 0o170000) != (new.mode & 0o170000)
    {
        append(out, name, before, None, hash)?;
        return append(out, name, None, after, hash);
    }
    let a = quote(b"a/", name);
    let b = quote(b"b/", name);
    writeln!(out, "diff --git {a} {b}")?;
    match (before, after) {
        (None, Some(new)) => writeln!(out, "new file mode {:06o}", new.mode)?,
        (Some(old), None) => writeln!(out, "deleted file mode {:06o}", old.mode)?,
        (Some(old), Some(new)) if old.mode != new.mode => {
            writeln!(out, "old mode {:06o}\nnew mode {:06o}", old.mode, new.mode)?;
        }
        _ => {}
    }
    let oid = |image: Option<&Image>| -> io::Result<gix::hash::ObjectId> {
        image.map_or(Ok(gix::hash::ObjectId::null(hash)), |image| {
            gix::objs::compute_hash(hash, gix::objs::Kind::Blob, &image.bytes)
                .map_err(io::Error::other)
        })
    };
    write!(out, "index {}..{}", oid(before)?, oid(after)?)?;
    if let (Some(old), Some(new)) = (before, after)
        && old.mode == new.mode
    {
        write!(out, " {:06o}", old.mode)?;
    }
    writeln!(out)?;
    let old = before.map_or(&[][..], |i| i.bytes.as_slice());
    let new = after.map_or(&[][..], |i| i.bytes.as_slice());
    if old == new {
        return Ok(());
    }
    if old.contains(&0)
        || new.contains(&0)
        || std::str::from_utf8(old).is_err()
        || std::str::from_utf8(new).is_err()
    {
        writeln!(out, "GIT binary patch")?;
        binary::literal(out, new)?;
        return binary::literal(out, old);
    }
    writeln!(
        out,
        "--- {}\n+++ {}",
        if before.is_some() { &a } else { "/dev/null" },
        if after.is_some() { &b } else { "/dev/null" }
    )?;
    let input = blob::InternedInput::new(old, new);
    let diff = blob::diff_with_slider_heuristics(blob::Algorithm::Histogram, &input);
    blob::UnifiedDiff::new(&diff, &input, Hunks(out), Default::default()).consume()?;
    Ok(())
}

struct Hunks<'a>(&'a mut Vec<u8>);
impl ConsumeHunk for Hunks<'_> {
    type Out = ();
    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(DiffLineKind, &[u8])],
    ) -> io::Result<()> {
        writeln!(self.0, "{header}")?;
        for (kind, line) in lines {
            if self.0.len().saturating_add(line.len() + 32) > super::LIMIT {
                return Err(crate::workspace::invalid("worktree patch exceeds 50 MiB"));
            }
            self.0.push(kind.to_prefix() as u8);
            self.0.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                self.0
                    .extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
        Ok(())
    }
    fn finish(self) {}
}

fn quote(prefix: &[u8], path: &[u8]) -> String {
    let mut result = String::from("\"");
    for &b in prefix.iter().chain(path) {
        match b {
            b'"' => result.push_str("\\\""),
            b'\\' => result.push_str("\\\\"),
            32..=126 => result.push(char::from(b)),
            _ => {
                use std::fmt::Write;
                write!(result, "\\{b:03o}").expect("String write");
            }
        }
    }
    result.push('"');
    result
}
