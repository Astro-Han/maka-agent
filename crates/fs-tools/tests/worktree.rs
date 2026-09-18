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

use maka_fs_tools::worktree::Worktrees;
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, atomic::AtomicBool},
};

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(args)
        .output()
        .expect("Git is the integration-test oracle only");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn repository(root: &Path, hash: &str) {
    fs::create_dir(root).unwrap();
    git(
        root,
        &["init", "--quiet", &format!("--object-format={hash}")],
    );
    git(root, &["config", "user.name", "Maka tests"]);
    git(root, &["config", "user.email", "tests@maka.invalid"]);
    fs::write(root.join("source.txt"), "first\nsecond\n").unwrap();
    fs::write(root.join("binary"), [0, 255, 13, 10]).unwrap();
    fs::write(root.join("deleted.txt"), "removed by child\n").unwrap();
    fs::write(root.join(".gitattributes"), "*.crlf text eol=crlf\n").unwrap();
    fs::write(root.join("lines.crlf"), "first\r\nsecond\r\n").unwrap();
    fs::write(root.join("executable"), "echo hello\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "--quiet", "-m", "base"]);
}

#[test]
fn linked_worktree_is_git_compatible_and_reopening_preserves_child_changes() {
    for hash in ["sha1", "sha256"] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("repository");
        repository(&source, hash);
        let root = temp.path().join("host");
        let manager = Worktrees::open(&root).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let binding = manager
            .plan(&source, &"1".repeat(64), cancel.clone())
            .unwrap();
        assert!(
            !binding.directory().exists(),
            "planning has no Git side effect"
        );
        manager.ensure(&binding, &cancel).unwrap();
        assert_eq!(
            git(binding.directory(), &["rev-parse", "HEAD"]).trim(),
            binding.base_commit()
        );
        assert!(git(binding.directory(), &["status", "--porcelain"]).is_empty());
        assert!(
            git(&source, &["worktree", "list", "--porcelain"])
                .contains(binding.directory().to_str().unwrap())
        );
        assert_eq!(
            fs::read(binding.directory().join("binary")).unwrap(),
            [0, 255, 13, 10]
        );
        git(binding.directory(), &["switch", "-c", "child-owned"]);
        fs::write(
            binding.directory().join("source.txt"),
            "committed child change\n",
        )
        .unwrap();
        git(binding.directory(), &["commit", "-am", "child"]);
        fs::write(
            binding.directory().join("source.txt"),
            "staged child change\n",
        )
        .unwrap();
        git(binding.directory(), &["add", "."]);
        fs::write(
            binding.directory().join("source.txt"),
            "unstaged child change\n",
        )
        .unwrap();
        fs::write(
            binding.directory().join("new.txt"),
            "untracked child file\n",
        )
        .unwrap();
        fs::write(binding.directory().join("binary"), [0, 1, 254, 13, 10, 9]).unwrap();
        fs::write(binding.directory().join("空 文件.txt"), "no final newline").unwrap();
        fs::write(binding.directory().join("empty"), "").unwrap();
        fs::remove_file(binding.directory().join("deleted.txt")).unwrap();
        fs::write(
            binding.directory().join("lines.crlf"),
            "changed\r\nsecond\r\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            fs::set_permissions(
                binding.directory().join("executable"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            symlink("source.txt", binding.directory().join("link")).unwrap();
        }
        let before = git(binding.directory(), &["status", "--porcelain"]);
        let index = git(binding.directory(), &["diff", "--cached"]);
        let saved = serde_json::to_vec(&binding).unwrap();
        let restored = serde_json::from_slice(&saved).unwrap();
        Worktrees::open(&root)
            .unwrap()
            .ensure(&restored, &cancel)
            .unwrap();
        assert_eq!(git(binding.directory(), &["status", "--porcelain"]), before);
        assert_eq!(git(binding.directory(), &["diff", "--cached"]), index);
        assert_eq!(
            git(binding.directory(), &["branch", "--show-current"]).trim(),
            "child-owned"
        );
        assert_eq!(
            fs::read_to_string(source.join("source.txt")).unwrap(),
            "first\nsecond\n"
        );
        assert!(git(&source, &["status", "--porcelain"]).is_empty());
        let patch = manager.capture_patch(&binding, cancel.clone()).unwrap();
        assert_eq!(
            git(binding.directory(), &["diff", "--cached"]),
            index,
            "export must not stage child edits"
        );
        let patch_path = temp.path().join("result.patch");
        fs::write(&patch_path, &patch).unwrap();
        git(
            &source,
            &["apply", "--index", "--binary", patch_path.to_str().unwrap()],
        );
        for file in [
            "source.txt",
            "binary",
            "new.txt",
            "空 文件.txt",
            "empty",
            "lines.crlf",
            "executable",
        ] {
            assert_eq!(
                fs::read(source.join(file)).unwrap(),
                fs::read(binding.directory().join(file)).unwrap(),
                "{file}"
            );
        }
        assert!(!source.join("deleted.txt").exists());
        #[cfg(unix)]
        {
            assert_eq!(
                fs::read_link(source.join("link")).unwrap(),
                Path::new("source.txt")
            );
            assert!(git(&source, &["ls-files", "--stage", "executable"]).starts_with("100755"));
        }
        git(
            &source,
            &[
                "apply",
                "--reverse",
                "--index",
                "--binary",
                patch_path.to_str().unwrap(),
            ],
        );
        assert!(
            git(&source, &["status", "--porcelain"]).is_empty(),
            "export is reversible against its base"
        );
    }
}

#[test]
fn worktree_recovery_rejects_dirty_sources_foreign_owners_and_missing_published_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("repository");
    repository(&source, "sha1");
    let root = temp.path().join("host");
    let manager = Worktrees::open(&root).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    fs::write(source.join("untracked"), "not part of HEAD").unwrap();
    assert!(
        manager
            .plan(&source, &"2".repeat(64), cancel.clone())
            .is_err()
    );
    fs::remove_file(source.join("untracked")).unwrap();
    let binding = manager
        .plan(&source, &"2".repeat(64), cancel.clone())
        .unwrap();
    let stopped = AtomicBool::new(true);
    assert_eq!(
        manager.ensure(&binding, &stopped).unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    manager.ensure(&binding, &cancel).unwrap();
    let allocation = binding.directory().parent().unwrap();
    // Interrupted final publication: ready staging must be reused, not checked out again.
    fs::write(
        binding.directory().join("source.txt"),
        "ready bytes preserved",
    )
    .unwrap();
    fs::rename(binding.directory(), allocation.join("checkout")).unwrap();
    manager.ensure(&binding, &cancel).unwrap();
    assert_eq!(
        fs::read_to_string(binding.directory().join("source.txt")).unwrap(),
        "ready bytes preserved"
    );
    let owner = allocation.join("maka-owner.json");
    let bytes = fs::read(&owner).unwrap();
    fs::write(&owner, "{}").unwrap();
    assert!(manager.ensure(&binding, &cancel).is_err());
    fs::write(&owner, bytes).unwrap();
    fs::rename(binding.directory(), allocation.join("user-recovered")).unwrap();
    assert!(
        manager.ensure(&binding, &cancel).is_err(),
        "never reset a published but missing directory"
    );
    assert!(!binding.directory().exists());
}
