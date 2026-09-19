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

use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::{Composition, Entry, Scope},
    contributions::Catalog,
    kernel::{Definition, Definitions, Kernel},
    services::Services,
};
use maka_runtime::{
    artifact::content_digest,
    execution::{WorkspaceProjection, WorkspaceTarget},
};
use maka_skills::{
    api::*,
    plugin::{Builtin, ID, PreferenceSnapshot, PreferenceStore, Skills},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(Default)]
struct Preferences {
    state: std::sync::Mutex<(u64, BTreeMap<String, maka_skills::Preference>)>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PreferenceStore for Preferences {
    fn compare_exchange(
        &self,
        expected: u64,
        reference: String,
        preference: maka_skills::Preference,
    ) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            let mut state = self.state.lock().unwrap();
            if state.0 != expected {
                return Ok(false);
            }
            state.0 += 1;
            state.1.insert(reference, preference);
            Ok(true)
        })
    }
    fn read(&self) -> BoxFuture<'_, Result<PreferenceSnapshot, String>> {
        Box::pin(async {
            let state = self.state.lock().unwrap();
            Ok(PreferenceSnapshot {
                revision: state.0,
                entries: state.1.clone(),
            })
        })
    }
}

async fn revision(skills: &Skills, workspace: &WorkspaceProjection) -> String {
    let input = CatalogInput::Start {
        context: WorkspaceContext {
            workspace: workspace.target.clone(),
        },
        view: CatalogView::Governance,
    };
    let CatalogResult::Page { revision, .. } =
        skills.query(&input, workspace.clone()).await.unwrap()
    else {
        panic!("first page");
    };
    revision
}
#[tokio::test]
async fn preview_confirms_raw_bytes_and_rejects_stale_or_invalid_sources_without_writing() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let installed = root.path().join("skills/review");
    let source = home.join(".maka/skill-sources/review");
    std::fs::create_dir_all(installed.join(".maka/baseline")).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    let original =
        "---\r\nname: Review\r\ndescription: Review code\r\n---\r\nOriginal instructions.\r\n";
    let replacement = format!(
        "---\nname: Review\ndescription: Review code\n---\n{}",
        "反斜线 \\\"\n".repeat(100)
    );
    std::fs::write(installed.join("SKILL.md"), original).unwrap();
    std::fs::write(installed.join(".maka/baseline/SKILL.md"), original).unwrap();
    std::fs::write(source.join("SKILL.md"), &replacement).unwrap();
    let lock = serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,"id":"review","sourceType":"managed",
        "sourceName":"local-library","sourceVersion":"1","sourceId":"review",
        "contentSha256":content_digest(original.as_bytes()),
        "sourceContentSha256":content_digest(original.as_bytes())
    }))
    .unwrap();
    std::fs::write(installed.join("skill.lock.json"), &lock).unwrap();
    let (_kernel, catalog) =
        activate(root.path(), Some(home), Arc::new(Preferences::default())).await;
    let skills = catalog
        .snapshot::<Skills>(&Scope::Profile)
        .entries
        .remove(ID)
        .unwrap();
    let workspace = WorkspaceProjection {
        target: WorkspaceTarget::HostPath {
            path: root.path().to_str().unwrap().into(),
        },
        host_cwd: root.path().to_str().unwrap().into(),
    };
    let mut input = PreviewInput {
        context: WorkspaceContext {
            workspace: workspace.target.clone(),
        },
        expected_revision: revision(&skills.value, &workspace).await,
        reference: "workspace:legacy:review".into(),
    };
    let result = skills
        .value
        .preview_update(&input, workspace.clone())
        .await
        .unwrap();
    let PreviewOutcome::Preview {
        expected_current_sha256,
        expected_source_sha256,
        current_snippet,
        source_snippet,
        current_truncated,
        source_truncated,
        has_managed_baseline,
        summary,
        ..
    } = result.outcome
    else {
        panic!("{result:?}")
    };
    assert_eq!(expected_current_sha256, content_digest(original.as_bytes()));
    assert_ne!(
        expected_current_sha256,
        content_digest(original.replace("\r\n", "\n").as_bytes())
    );
    assert_eq!(
        expected_source_sha256,
        content_digest(replacement.as_bytes())
    );
    assert_eq!(current_snippet, original.replace("\r\n", "\n"));
    assert!(!current_truncated);
    assert!(source_truncated && has_managed_baseline);
    assert_eq!(source_snippet.lines().count(), 80);
    assert!(summary.changed_line_count > 90);
    assert_eq!(
        std::fs::read(installed.join("SKILL.md")).unwrap(),
        original.as_bytes()
    );
    assert_eq!(
        std::fs::read(installed.join("skill.lock.json")).unwrap(),
        lock
    );

    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: Review\n---\nInvalid metadata",
    )
    .unwrap();
    let stale = skills
        .value
        .preview_update(&input, workspace.clone())
        .await
        .unwrap();
    assert!(matches!(
        stale.outcome,
        PreviewOutcome::RevisionConflict { .. }
    ));
    input.expected_revision = revision(&skills.value, &workspace).await;
    let invalid = skills
        .value
        .preview_update(&input, workspace.clone())
        .await
        .unwrap();
    assert!(matches!(
        invalid.outcome,
        PreviewOutcome::Rejected {
            reason: PreviewRejection::SourceInvalid
        }
    ));
    std::fs::remove_file(source.join("SKILL.md")).unwrap();
    input.expected_revision = revision(&skills.value, &workspace).await;
    let missing = skills
        .value
        .preview_update(&input, workspace)
        .await
        .unwrap();
    assert!(matches!(
        missing.outcome,
        PreviewOutcome::Rejected {
            reason: PreviewRejection::SourceMissing
        }
    ));
}

async fn activate(
    root: &std::path::Path,
    home: Option<std::path::PathBuf>,
    preferences: Arc<Preferences>,
) -> (Kernel, Catalog) {
    let catalog = Catalog::default();
    let definitions = Definitions::from([(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: "test".into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Builtin {
                client: None,
                state_root: root.into(),
                home,
                preferences,
            }),
        }),
    )]);
    let mut entry = Entry::new(ID).unwrap();
    entry.package_id = Some(ID.into());
    let mut kernel = Kernel::new(Services::default(), catalog.clone())
        .with_data(maka_plugins::storage::Directories::open(root).unwrap());
    kernel
        .configure(
            &Composition {
                roots: BTreeMap::from([(Scope::Profile, vec![entry])]),
            },
            definitions,
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !kernel.tick().unwrap().converged {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    (kernel, catalog)
}
#[tokio::test]
async fn accepted_preference_write_outlives_its_waiter_and_plugin_retirement() {
    let root = tempfile::tempdir().unwrap();
    let installed = root.path().join("skills/review");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::write(
        installed.join("SKILL.md"),
        "---\nname: Review\ndescription: Review code\n---\nReview.",
    )
    .unwrap();
    let preferences = Arc::new(Preferences::default());
    let (mut kernel, catalog) = activate(root.path(), None, preferences.clone()).await;
    let skills = catalog
        .snapshot::<Skills>(&Scope::Profile)
        .entries
        .remove(ID)
        .unwrap()
        .value;
    let workspace = WorkspaceProjection {
        target: WorkspaceTarget::HostPath {
            path: root.path().to_str().unwrap().into(),
        },
        host_cwd: root.path().to_str().unwrap().into(),
    };
    let input = MutateInput {
        context: WorkspaceContext {
            workspace: workspace.target.clone(),
        },
        expected_revision: revision(&skills, &workspace).await,
        mutation: Mutation::SetPinned {
            reference: "workspace:legacy:review".into(),
            pinned: true,
        },
    };
    let caller = tokio::spawn(async move { skills.mutate(input, workspace).await });
    tokio::time::timeout(Duration::from_secs(2), preferences.entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    kernel
        .configure(&Composition::default(), Definitions::default())
        .unwrap();
    kernel.tick().unwrap();
    assert!(
        !kernel.status().cleanup_complete,
        "the accepted commit owns the retirement lease"
    );
    preferences.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !kernel.tick().unwrap().converged {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let committed = preferences.read().await.unwrap();
    assert_eq!(committed.revision, 1);
    assert!(committed.entries["workspace:legacy:review"].pinned);
    assert!(
        catalog
            .snapshot::<Skills>(&Scope::Profile)
            .entries
            .is_empty()
    );
}
