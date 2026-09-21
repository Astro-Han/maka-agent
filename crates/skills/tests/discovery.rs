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

use maka_skills::SkillFailureReason;
use maka_skills::{
    Catalog, DiscoveryFailure, HostCapabilities, IssueCode, Preference, Preferences, ScanError,
    Source, scan,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
use tokio_util::sync::CancellationToken;

fn owner() -> maka_plugins::fiber::Fiber {
    let owner = maka_plugins::fiber::Fiber::new(
        "example.discovery",
        "reader",
        maka_plugins::composition::Scope::Profile,
    )
    .unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    owner.publish().unwrap();
    owner
}
async fn view(
    owner: &maka_plugins::fiber::Fiber,
    path: &Path,
) -> maka_plugins::filesystem::ReadDirectory {
    maka_plugins::filesystem::ReadRoot::open(path)
        .await
        .unwrap()
        .bind(owner.context(), CancellationToken::new())
}

fn skill(path: &Path, name: &str, requirements: &str, body: &str) -> Vec<u8> {
    fs::create_dir_all(path).unwrap();
    let bytes = format!(
        "---\r\nname: {name}\r\ndescription: local instructions\r\n{requirements}---\r\n{body}"
    )
    .into_bytes();
    fs::write(path.join("SKILL.md"), &bytes).unwrap();
    bytes
}

#[tokio::test]
async fn snapshot_precedence_preferences_and_capabilities_preserve_selected_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let workspace = temporary.path().join("workspace");
    let home = temporary.path().join("home");
    for (path, name) in [
        (project.join(".agents/skills/REVIEW"), "Shadow"),
        (workspace.join("skills/review"), "Legacy"),
        (home.join(".maka/skills/duplicate"), "Primary"),
    ] {
        skill(&path, name, "", "unused");
    }
    let original = skill(
        &project.join(".maka/skills/review"),
        "Primary",
        "required-tools: Read\r\nallowed-tools: UnregisteredHint\r\n",
        "frozen\r\n  instructions",
    );
    skill(
        &home.join(".agents/skills/alias"),
        "review",
        "",
        "wrong display-name match",
    );
    skill(
        &project.join(".maka/skills/network"),
        "workspace:legacy:review",
        "required-capabilities: network\r\n",
        "network",
    );
    skill(
        &project.join(".maka/skills/large"),
        "Large",
        "",
        &"😀".repeat(24_001),
    );
    skill(
        &project.join(".maka/skills/invalid"),
        "Invalid",
        "required-tools: 42\r\n",
        "invalid",
    );
    let owner = owner();
    let project_files = view(&owner, &project).await;
    let workspace_files = view(&owner, &workspace).await;
    let home_files = view(&owner, &home).await;
    let sources = Source::standard(&project_files, &workspace_files, Some(&home_files));
    let snapshot = scan(&sources, &CancellationToken::new()).await.unwrap();
    assert!(snapshot.diagnostics.is_empty());
    assert_eq!(snapshot.inventory.len(), 7);
    assert_eq!(snapshot.rejected.len(), 1);
    let review = snapshot
        .inventory
        .iter()
        .find(|skill| skill.location.reference == "project:maka:review")
        .unwrap();
    assert_eq!(
        review.content_sha256,
        maka_runtime::artifact::content_digest(&original)
    );
    assert!(
        review
            .document
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::DuplicateName)
    );
    assert_eq!(
        snapshot
            .inventory
            .iter()
            .filter(|skill| skill.shadowed_by.as_deref() == Some("project:maka:review"))
            .count(),
        2
    );
    fs::write(
        project.join(".maka/skills/review/SKILL.md"),
        "replaced after scan",
    )
    .unwrap();
    let mut host = HostCapabilities::default();
    host.tools.insert("Read".into());
    let preferences = Preferences::Available(BTreeMap::new());
    let mut capable = HostCapabilities::default();
    capable.capabilities.insert("network".into());
    let capable_catalog = Catalog {
        discovery: &snapshot,
        preferences: &preferences,
        host: &capable,
    };
    assert_eq!(
        capable_catalog.load("workspace:legacy:review").unwrap_err(),
        SkillFailureReason::NotFound
    );
    let catalog = Catalog {
        discovery: &snapshot,
        preferences: &preferences,
        host: &host,
    };
    let loaded = catalog.load(" ReViEw ").unwrap();
    let search = catalog.search(" \u{feff}rEvIeW\t", 1);
    assert_eq!(search.query, "review");
    assert_eq!(search.matches[0].skill.reference, "project:maka:review");
    assert_eq!(search.matched_count, 2);
    assert!(search.truncated);
    let pinned = Preferences::Available(BTreeMap::from([(
        "project:maka:large".into(),
        Preference {
            enabled: true,
            pinned: true,
        },
    )]));
    let pinned_catalog = Catalog {
        preferences: &pinned,
        ..catalog
    };
    assert_eq!(
        pinned_catalog
            .search("unrelated vocabulary", 8)
            .matched_count,
        0,
        "pinning does not create a match"
    );
    let long_query = catalog.search(&"😀".repeat(300), 8);
    assert_eq!(long_query.query.encode_utf16().count(), 512);
    assert!(long_query.query_truncated);
    for budget in [0, 100, 600, 18_000] {
        let prompt = catalog.prompt(budget);
        assert!(prompt.len() <= budget);
        assert!(!prompt.contains("frozen"));
        if budget == 600 {
            assert!(prompt.contains("additional enabled skill(s) omitted"));
        }
    }
    assert_eq!(loaded.skill.location.reference, "project:maka:review");
    assert_eq!(loaded.instructions, "frozen\n  instructions");
    assert_eq!(
        loaded.relative_path,
        Path::new(".maka/skills/review/SKILL.md")
    );
    assert_eq!(
        catalog.load("network").unwrap_err(),
        SkillFailureReason::HostIncompatible
    );
    assert_eq!(
        catalog.load("bad\0request").unwrap_err(),
        SkillFailureReason::InvalidName
    );
    let long = catalog.load("large").unwrap();
    assert!(long.truncated);
    assert_eq!(
        long.instructions,
        format!("{}\n[skill truncated]", "😀".repeat(23_975))
    );
    let disabled = Preferences::Available(BTreeMap::from([(
        "project:maka:review".into(),
        Preference {
            enabled: false,
            pinned: true,
        },
    )]));
    let catalog = Catalog {
        preferences: &disabled,
        ..catalog
    };
    assert_eq!(
        catalog.load("review").unwrap_err(),
        SkillFailureReason::Disabled
    );
    assert_eq!(
        catalog.load("project:maka:review").unwrap_err(),
        SkillFailureReason::Disabled
    );
    assert_eq!(
        catalog.load("user:agents:alias").unwrap().instructions,
        "wrong display-name match"
    );
    let unavailable = Preferences::Unavailable;
    let catalog = Catalog {
        preferences: &unavailable,
        ..catalog
    };
    assert_eq!(catalog.available().count(), 0);
    assert_eq!(
        catalog.load("review").unwrap_err(),
        SkillFailureReason::ResolutionFailed
    );
    let missing_library = maka_skills::source_catalog(
        &workspace_files,
        Some(&home_files),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(missing_library.managed.discovery.inventory.is_empty());
    assert_eq!(missing_library.bundled[0].id, "computer-use");
    skill(
        &home.join(".maka/skill-sources/source"),
        "Source",
        "",
        "Not installed",
    );
    fs::create_dir_all(workspace.join("skills/computer-use")).unwrap();
    let library = maka_skills::source_catalog(
        &workspace_files,
        Some(&home_files),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        library.managed.discovery.inventory[0]
            .document
            .manifest
            .name,
        "Source"
    );
    assert!(
        library.publication.occupied.contains("computer-use"),
        "empty directories are still occupied"
    );
    let alias = workspace.join("skills/alias");
    let installed_body = skill(
        &alias,
        "Installed alias",
        "",
        "locally modified instructions",
    );
    let hash = format!("sha256:{}", "a".repeat(64));
    let lock = serde_json::json!({
        "schemaVersion": 1, "id": "alias", "sourceType": "managed",
        "sourceName": "local-library", "sourceVersion": "1", "sourceId": "SOURCE",
        "contentSha256": hash, "sourceContentSha256": hash,
    });
    let lock_path = alias.join("skill.lock.json");
    let inspect = || async {
        maka_skills::source_catalog(
            &workspace_files,
            Some(&home_files),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    };
    assert!(
        !inspect()
            .await
            .installed_managed_sources()
            .contains("source")
    );
    fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let installed = inspect().await;
    assert!(
        installed.installed_managed_sources().contains("source"),
        "local body changes do not erase a valid origin"
    );
    assert!(
        !installed.publication.occupied.contains("source"),
        "origin and directory occupancy are different facts"
    );
    let origin = &installed.publication.origins["workspace:legacy:alias"];
    assert_eq!(
        origin.lock_sha256,
        Some(maka_runtime::artifact::content_digest(
            &serde_json::to_vec(&lock).unwrap()
        ))
    );
    assert_ne!(
        maka_runtime::artifact::content_digest(&installed_body),
        hash
    );
    for (field, value) in [
        ("id", serde_json::json!("other")),
        ("schemaVersion", serde_json::json!(2)),
        ("sourceName", serde_json::json!("untrusted")),
        ("sourceId", serde_json::json!("../source")),
        (
            "sourceContentSha256",
            serde_json::json!(format!("sha256:{}", "b".repeat(64))),
        ),
    ] {
        let mut invalid = lock.clone();
        invalid[field] = value;
        fs::write(&lock_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(
            !inspect()
                .await
                .installed_managed_sources()
                .contains("source"),
            "invalid {field} must not create an installation alias"
        );
    }
    let mut invalid_utf8 = serde_json::to_vec(&lock).unwrap();
    invalid_utf8.pop();
    invalid_utf8.extend_from_slice(b",\"ignored\":\"\xff\"}");
    fs::write(&lock_path, &invalid_utf8).unwrap();
    let invalid = inspect().await;
    assert!(!invalid.installed_managed_sources().contains("source"));
    assert_eq!(
        invalid.publication.origins["workspace:legacy:alias"].lock_sha256,
        Some(maka_runtime::artifact::content_digest(&invalid_utf8))
    );
    fs::remove_file(&lock_path).unwrap();
    directory_link(&home, &lock_path);
    assert!(matches!(
        inspect().await.publication.origins["workspace:legacy:alias"].status,
        maka_skills::OriginStatus::Invalid(maka_skills::OriginFailure::UnsafePath)
    ));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        scan(&sources, &cancellation).await.unwrap_err(),
        ScanError::Cancelled
    );
    assert!(matches!(
        maka_skills::source_catalog(&workspace_files, Some(&home_files), &cancellation).await,
        Err(maka_skills::SourceCatalogError::Scan(ScanError::Cancelled))
    ));
}

#[tokio::test]
async fn discovery_reads_only_contained_regular_files_and_reports_source_failures() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    let outside = temporary.path().join("outside");
    skill(&root.join(".maka/skills/normal"), "Normal", "", "local");
    skill(&root.join("store/linked"), "Linked", "", "inside");
    skill(&outside.join("skills/leak"), "Leak", "", "outside");
    directory_link(
        &root.join("store/linked"),
        &root.join(".maka/skills/inside"),
    );
    directory_link(
        &root.join(".maka/skills/inside"),
        &root.join(".maka/skills/chain"),
    );
    directory_link(
        &outside.join("skills/leak"),
        &root.join(".maka/skills/escape"),
    );
    directory_link(&outside, &root.join(".agents"));
    fs::create_dir_all(root.join("skills/EmptyCase")).unwrap();
    fs::write(root.join("skills/file-only"), "not a skill directory").unwrap();
    directory_link(
        &root.join("skills/EmptyCase"),
        &root.join("skills/empty-alias"),
    );
    let owner = owner();
    let root_files = view(&owner, &root).await;
    let governance =
        maka_skills::governance_catalog(&root_files, &root_files, None, &CancellationToken::new())
            .await
            .unwrap();
    assert_eq!(
        governance
            .publication
            .empty
            .iter()
            .map(|s| s.reference.as_str())
            .collect::<Vec<_>>(),
        ["workspace:legacy:EmptyCase"],
        "only real directories retain exact governance identity; files and empty links only occupy names",
    );
    assert!(governance.publication.occupied.contains("file-only"));
    assert!(governance.publication.occupied.contains("empty-alias"));
    fs::create_dir_all(root.join(".maka/skills/large")).unwrap();
    fs::File::create(root.join(".maka/skills/large/SKILL.md"))
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    #[cfg(unix)]
    {
        let linked_file = root.join(".maka/skills/linked-file");
        fs::create_dir_all(&linked_file).unwrap();
        std::os::unix::fs::symlink(
            root.join(".maka/skills/normal/SKILL.md"),
            linked_file.join("SKILL.md"),
        )
        .unwrap();
        let pipe = root.join(".maka/skills/pipe");
        fs::create_dir_all(&pipe).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(pipe.join("SKILL.md"))
            .status()
            .unwrap();
        assert!(status.success());
    }
    let snapshot = scan(
        &Source::standard(&root_files, &root_files, None),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let ids: BTreeSet<_> = snapshot
        .inventory
        .iter()
        .map(|skill| skill.location.id.as_str())
        .collect();
    assert_eq!(ids, BTreeSet::from(["chain", "inside", "normal"]));
    for (relative, reason) in [
        (".maka/skills/escape", DiscoveryFailure::BlockedPath),
        (".agents/skills", DiscoveryFailure::BlockedPath),
        (".maka/skills/large", DiscoveryFailure::SourceTooLarge),
    ] {
        assert!(
            snapshot
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path == root.join(relative)
                    && diagnostic.reason == reason),
            "{relative}: {:?}",
            snapshot.diagnostics
        );
    }
    directory_link(&outside, &root.join(".maka/skill-sources"));
    assert!(
        matches!(
            maka_skills::source_catalog(&root_files, Some(&root_files), &CancellationToken::new())
                .await,
            Err(maka_skills::SourceCatalogError::Read(
                DiscoveryFailure::BlockedPath
            ))
        ),
        "an unsafe configured library must not become an empty catalog"
    );
    #[cfg(unix)]
    for relative in [".maka/skills/linked-file", ".maka/skills/pipe"] {
        assert!(
            snapshot
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.path == root.join(relative)
                    && diagnostic.reason == DiscoveryFailure::BlockedPath)
        );
    }
}

#[cfg(unix)]
fn directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}
#[cfg(windows)]
fn directory_link(target: &Path, link: &Path) {
    // Junctions exercise Windows directory reparse boundaries without requiring
    // Developer Mode or changing the machine's symlink privilege policy.
    // mklink treats forward slashes as switches, unlike Rust filesystem APIs.
    let link: std::path::PathBuf = link.components().collect();
    let target: std::path::PathBuf = target.components().collect();
    let output = std::process::Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
