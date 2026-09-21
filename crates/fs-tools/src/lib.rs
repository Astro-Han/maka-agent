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

mod edit_index;
mod edit_match;
pub use edit_match::EditMatchStrategy;
pub mod directory;
mod search;
pub mod workspace;
pub mod worktree;
pub use search::{GLOB_DESCRIPTION, GLOB_NAME, glob_schema};
mod grep;
pub use grep::{GREP_DESCRIPTION, GREP_NAME, grep_schema};
mod mutation;
mod patch;
mod scoped;
mod write;
mod write_target;

pub use mutation::{
    EDIT_DESCRIPTION, EDIT_NAME, WRITE_DESCRIPTION, WRITE_NAME, edit_schema, write_schema,
};
pub use patch::{PATCH_DESCRIPTION, PATCH_NAME, patch_schema};
pub use write::{MutationExecutor, WriteCoordinator, WriteScope};

use maka_runtime::{
    read::{ReadInput, ReadPage, ReadRequest},
    tools::{ToolError, ToolExecutor, ToolFuture},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const READ_NAME: &str = "Read";
pub const READ_DESCRIPTION: &str = "Read a bounded page of UTF-8 text or a PNG/JPEG/GIF/WebP image within the Session's allowed filesystem roots. Offset is zero-based lines; limit is a positive line count. Pass a non-null next object to Read to continue; a changed source invalidates the continuation. Images ignore offset/limit and must be at most 5 MiB with dimensions at most 2000 pixels.";

pub fn read_schema() -> Value {
    schemars::schema_for!(ReadInput).into()
}

/// Trusted Host authority, never deserialized from tool arguments.
/// Directory capabilities enforce reads; this is not an OS process sandbox.
pub enum ReadScope {
    Restricted { roots: Vec<PathBuf> },
    Unrestricted,
}

#[derive(Clone, Copy)]
pub struct ReadLimits {
    pub max_source_bytes: usize,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 1024 * 1024,
        }
    }
}

/// Bytes remain internal until the Host commits the image snapshot and result.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadOutput {
    Text(ReadPage),
    Image { bytes: Vec<u8>, mime_type: String },
}

#[derive(Clone)]
pub struct ReadExecutor {
    authority: Arc<scoped::Authority>,
    limits: ReadLimits,
}

impl ReadExecutor {
    /// Use the exact directory already validated by an embedding's file grant.
    pub fn from_directory(
        path: PathBuf,
        directory: cap_std::fs::Dir,
        limits: ReadLimits,
    ) -> Result<Self, ToolError> {
        if limits.max_source_bytes == 0 || limits.max_source_bytes.checked_add(1).is_none() {
            return Err(failed("Read byte limits must be positive and bounded"));
        }
        Ok(Self {
            authority: Arc::new(scoped::Authority::from_directory(path, directory)?),
            limits,
        })
    }
    pub fn new(
        cwd: impl AsRef<Path>,
        scope: ReadScope,
        limits: ReadLimits,
    ) -> Result<Self, ToolError> {
        if limits.max_source_bytes == 0 || limits.max_source_bytes.checked_add(1).is_none() {
            return Err(failed("Read byte limits must be positive and bounded"));
        }
        Ok(Self {
            authority: Arc::new(scoped::Authority::new(cwd.as_ref(), scope)?),
            limits,
        })
    }

    pub async fn read(
        &self,
        input: ReadRequest,
        cancellation: CancellationToken,
    ) -> Result<ReadOutput, ToolError> {
        let authority = self.authority.clone();
        let limits = self.limits;
        if input.path().contains("://") {
            return Err(failed("Filesystem Read requires a file path"));
        }
        // Always join blocking I/O: cancellation must not detach unsettled work.
        tokio::task::spawn_blocking(move || authority.read(input, limits, cancellation))
            .await
            .map_err(|e| failed(format!("Read worker failed: {e}")))?
    }
}

impl ToolExecutor for ReadExecutor {
    fn names(&self) -> Vec<String> {
        vec![READ_NAME.into(), GLOB_NAME.into(), GREP_NAME.into()]
    }

    fn invoke(&self, name: String, input: Value, cancellation: CancellationToken) -> ToolFuture {
        let executor = self.clone();
        Box::pin(async move {
            if name == GLOB_NAME {
                return executor.glob(input, cancellation).await;
            }
            if name == GREP_NAME {
                return executor.grep(input, cancellation).await;
            }
            if name != READ_NAME {
                return Err(failed("unsupported tool"));
            }
            let input: ReadInput =
                serde_json::from_value(input).map_err(|e| failed(e.to_string()))?;
            let request = input.resolve().map_err(|e| failed(e.to_string()))?;
            match executor.read(request, cancellation).await? {
                ReadOutput::Text(page) => Ok(serde_json::to_value(page).expect("ReadPage is JSON")),
                ReadOutput::Image { .. } => Err(failed(
                    "Read image requires a durable journal snapshot; use the Host typed Read path",
                )),
            }
        })
    }
}

fn failed(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}

#[cfg(all(test, unix))]
mod image_tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    fn request(path: impl AsRef<Path>) -> ReadRequest {
        ReadInput {
            path: path.as_ref().to_str().unwrap().into(),
            offset: Some(99),
            limit: std::num::NonZeroUsize::new(1),
        }
        .resolve()
        .unwrap()
    }

    // Header fixtures exercise the promised dimensions probe, not pixel decoding.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes
    }

    fn executor(root: &Path) -> ReadExecutor {
        ReadExecutor::new(
            root,
            ReadScope::Restricted {
                roots: vec![root.to_owned()],
            },
            // Images have an independent cap, including when text limits are tiny.
            ReadLimits {
                max_source_bytes: 1,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn image_formats_use_content_mime_and_ignore_line_slicing() {
        let temp = tempfile::tempdir().unwrap();
        let read = executor(temp.path());
        for (path, bytes, mime_type) in [
            ("mismatch.JpEg", png(1, 1), "image/png"),
            ("photo.jpg", b"\xff\xd8\xff\xc0\0\x0b\x08\0\x01\0\x02\x01\x01\x11\0".to_vec(), "image/jpeg"),
            ("motion.gif", b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff\x2c\0\0\0\0\x01\0\x01\0\0\x02\x02\x44\x01\0\x3b".to_vec(), "image/gif"),
            ("picture.webp", b"RIFF\x16\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec(), "image/webp"),
        ] {
            fs::write(temp.path().join(path), &bytes).unwrap();
            assert_eq!(read.read(request(path), CancellationToken::new()).await.unwrap(),
                ReadOutput::Image { bytes, mime_type: mime_type.into() });
        }
        let error = read
            .invoke(
                READ_NAME.into(),
                serde_json::json!({"path":"mismatch.JpEg"}),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("durable journal snapshot"));
    }

    #[tokio::test]
    async fn image_bounds_and_invalid_headers_are_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let read = executor(temp.path());
        let path = temp.path().join("image.png");
        let mut at_cap = png(2000, 2000);
        at_cap.resize(5 * 1024 * 1024, 0);
        fs::write(&path, &at_cap).unwrap();
        assert!(
            read.read(request(&path), CancellationToken::new())
                .await
                .is_ok()
        );
        at_cap.push(0);
        let mut corrupt_png = png(1, 1);
        corrupt_png[4..8].copy_from_slice(b"oops");
        for bytes in [
            at_cap,
            corrupt_png,
            b"GIF8xx\x01\0\x01\0\0\0".to_vec(),
            png(2001, 1),
            png(1, 2001),
            png(0, 1),
            png(1, 0),
            b"not an image".to_vec(),
            b"\x89PNG".to_vec(),
            b"\0\0\0\x18ftypavif\0\0\0\0avifmif1".to_vec(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(
                read.read(request(&path), CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        fs::write(&path, png(1, 1)).unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(read.read(request(path), cancellation).await.is_err());
    }

    #[tokio::test]
    async fn image_read_keeps_capability_and_requested_extension_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("workspace");
        fs::create_dir(&root).unwrap();
        let bytes = png(1, 1);
        fs::write(root.join("data"), &bytes).unwrap();
        fs::write(base.join("outside.png"), &bytes).unwrap();
        symlink("data", root.join("alias.png")).unwrap();
        symlink("../outside.png", root.join("escape.png")).unwrap();
        let alias = base.join("root-alias");
        symlink(&root, &alias).unwrap();
        let read = ReadExecutor::new(
            &root,
            ReadScope::Restricted {
                roots: vec![alias.clone()],
            },
            ReadLimits::default(),
        )
        .unwrap();
        assert_eq!(
            read.read(request(alias.join("alias.png")), CancellationToken::new())
                .await
                .unwrap(),
            ReadOutput::Image {
                bytes: bytes.clone(),
                mime_type: "image/png".into()
            }
        );
        for path in [
            base.join("outside.png"),
            root.join("escape.png"),
            root.clone(),
        ] {
            assert!(
                read.read(request(path), CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        for name in ["data", "data.svg", "data.avif", "data.pdf"] {
            fs::write(root.join(name), &bytes).unwrap();
            assert!(
                read.read(request(name), CancellationToken::new())
                    .await
                    .is_err()
            );
        }
        // Replacing the ambient root cannot redirect an already captured capability.
        fs::rename(&root, base.join("captured")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("alias.png"), "attacker").unwrap();
        assert_eq!(
            read.read(request(alias.join("alias.png")), CancellationToken::new())
                .await
                .unwrap(),
            ReadOutput::Image {
                bytes,
                mime_type: "image/png".into()
            }
        );
    }
}
