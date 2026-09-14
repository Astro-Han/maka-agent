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

#![cfg(unix)]

use maka_fs_tools::instructions::{InstructionFile, read_instruction_files};
use std::{
    ffi::CString,
    fs,
    os::unix::{ffi::OsStrExt, fs::symlink},
};

#[test]
fn directory_capability_contains_aliases_and_skips_nonregular_or_oversized_sources() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "OUTSIDE").unwrap();
    fs::write(root.path().join("AGENTS.md"), b"\xef\xbb\xbf Rule\0\n").unwrap();
    fs::write(root.path().join("CLAUDE.md"), "Rule").unwrap();
    symlink(outside.path().join("secret"), root.path().join("GEMINI.md")).unwrap();
    let instructions = read_instruction_files(root.path());
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].file, InstructionFile::Agents);
    assert_eq!(instructions[0].text, "Rule");
    fs::remove_file(root.path().join("GEMINI.md")).unwrap();
    symlink(root.path().join("AGENTS.md"), root.path().join("GEMINI.md")).unwrap();
    assert_eq!(read_instruction_files(root.path()).len(), 1);
    fs::remove_file(root.path().join("CLAUDE.md")).unwrap();
    let fifo = CString::new(root.path().join("CLAUDE.md").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert_eq!(read_instruction_files(root.path()).len(), 1);
    fs::write(root.path().join("AGENTS.md"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(read_instruction_files(root.path()).is_empty());
}

#[test]
fn cleaned_full_content_is_deduplicated_before_scalar_truncation() {
    let root = tempfile::tempdir().unwrap();
    let prefix = "🦀".repeat(6000);
    fs::write(root.path().join("AGENTS.md"), format!("{prefix}a")).unwrap();
    fs::write(root.path().join("CLAUDE.md"), format!("{prefix}b")).unwrap();
    fs::write(
        root.path().join("GEMINI.md"),
        [b'R', 0xff, b'\t', b'\r', b'\n', 0x1f, b'X'],
    )
    .unwrap();
    let instructions = read_instruction_files(root.path());
    assert_eq!(instructions.len(), 3);
    for instruction in &instructions[..2] {
        assert_eq!(instruction.text, prefix);
        assert!(instruction.truncated);
    }
    assert_eq!(instructions[2].text, "R\u{fffd}\t\r\nX");
    assert!(!instructions[2].truncated);
}
