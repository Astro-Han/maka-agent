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
use maka_runtime::input::MessageInput;
use maka_skills::plugin::{
    Builtin, ID, InputPreparation, PreferenceSnapshot, PreferenceStore, Skills, Snapshot,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct Preferences(AtomicU64);
impl PreferenceStore for Preferences {
    fn compare_exchange(
        &self,
        _: u64,
        _: String,
        _: maka_skills::Preference,
    ) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async { Err("read-only fixture".into()) })
    }
    fn read(&self) -> BoxFuture<'_, Result<PreferenceSnapshot, String>> {
        Box::pin(async {
            Ok(PreferenceSnapshot {
                revision: self.0.load(Ordering::SeqCst),
                entries: BTreeMap::new(),
            })
        })
    }
}

async fn converge(kernel: &mut Kernel) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if kernel.tick().unwrap().converged {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn message(text: &str) -> MessageInput {
    serde_json::from_value(serde_json::json!({"text": text})).unwrap()
}

#[tokio::test]
async fn snapshots_preserve_instructions_while_retirement_fences_prepared_admission() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let skill = workspace.join(".maka/skills/review");
    std::fs::create_dir_all(&skill).unwrap();
    let file = skill.join("SKILL.md");
    std::fs::write(
        &file,
        "---\nname: Review\ndescription: Review code\n---\nOriginal instructions.",
    )
    .unwrap();
    let preferences = Arc::new(Preferences::default());
    let catalog = Catalog::default();
    catalog.host_only::<Skills>().unwrap();
    catalog.reserve_for::<Skills>(ID, ID).unwrap();
    let definitions = Definitions::from([(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: "builtin".into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Builtin {
                client: None,
                state_root: root.path().to_owned(),
                home: None,
                preferences: preferences.clone(),
            }),
        }),
    )]);
    let mut entry = Entry::new(ID).unwrap();
    entry.package_id = Some(ID.into());
    let mut composition = Composition {
        roots: BTreeMap::from([(Scope::Profile, vec![entry])]),
    };
    let mut kernel = Kernel::new(Services::default(), catalog.clone())
        .with_data(maka_plugins::storage::Directories::open(root.path()).unwrap());
    kernel.configure(&composition, definitions.clone()).unwrap();
    converge(&mut kernel).await;
    let contribution = catalog
        .snapshot::<Skills>(&Scope::Profile)
        .entries
        .remove(ID)
        .unwrap();
    let activation = contribution.owner.identity().unwrap().activation;
    let invocation = maka_runtime::event::Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let tools = maka_tool_catalog::availability::Availability::new(
        maka_tool_catalog::ToolCatalog::default()
            .with_discovery()
            .with_plugins(catalog.clone(), Scope::Session("session".into()), None)
            .unwrap(),
    );
    let capture = || {
        tools.capture(
            invocation.clone(),
            workspace.to_string_lossy().into_owned(),
            Default::default(),
        )
    };
    let (first_tools, _, first_context) = capture().await.unwrap();
    assert_eq!(first_tools.snapshot().names(), ["Skill", "SkillSearch"]);
    assert_eq!(
        first_context.contexts.len(),
        1,
        "shared binding publishes context only once"
    );
    assert!(first_context.contexts[0].contains("Review code"));
    let restricted = maka_tool_catalog::availability::Availability::new(
        maka_tool_catalog::ToolCatalog::default()
            .with_plugins(
                catalog.clone(),
                Scope::Session("session".into()),
                Some(Default::default()),
            )
            .unwrap(),
    );
    let (restricted, _, context) = restricted
        .capture(
            invocation.clone(),
            workspace.to_string_lossy().into_owned(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(restricted.snapshot().names().is_empty());
    assert!(
        context.contexts.is_empty(),
        "no hidden plugin context outside the capability ceiling"
    );
    let snapshot = contribution
        .value
        .capture(workspace.to_str().unwrap(), Default::default())
        .await
        .unwrap();
    let mut input = message("/skill:review Check this change.");
    let InputPreparation::Ready {
        skill_invocation, ..
    } = snapshot.prepare(&mut input, &[]).unwrap()
    else {
        panic!("Skill should resolve");
    };
    assert_eq!(skill_invocation.loaded.len(), 1);
    assert!(input.text.contains("Original instructions."));
    let accepted = serde_json::to_value((&input, &skill_invocation)).unwrap();
    let prepared = maka_plugins::input::prepare(
        &catalog,
        &Scope::Session("session".into()),
        maka_plugins::input::Request {
            session_id: "session".into(),
            cwd: workspace.to_string_lossy().into_owned(),
            content: message("/skill:review Check this change."),
            selections: Default::default(),
            tools: Default::default(),
            cancellation: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(prepared.content.text, input.text);
    std::fs::write(
        &file,
        "---\nname: Review\ndescription: Updated review\n---\nReplacement instructions.",
    )
    .unwrap();
    let (_, _, updated_context) = capture().await.unwrap();
    assert!(updated_context.contexts[0].contains("Updated review"));
    assert!(
        first_context.contexts[0].contains("Review code"),
        "a retry retains the old context"
    );
    assert_ne!(
        first_context.sources[0].revision,
        updated_context.sources[0].revision
    );
    let mut same_step = message("/skill:review Check this change.");
    snapshot.prepare(&mut same_step, &[]).unwrap();
    assert_eq!(input, same_step);
    let admitted = prepared.admit().unwrap();
    composition.roots.get_mut(&Scope::Profile).unwrap()[0].disabled = true;
    kernel.configure(&composition, definitions.clone()).unwrap();
    kernel.tick().unwrap();
    assert!(
        prepared.admit().is_err(),
        "no new work from a retained snapshot"
    );
    assert!(
        !kernel.status().cleanup_complete,
        "admitted work retains its cleanup lease"
    );
    assert_eq!(
        accepted,
        serde_json::to_value((&input, &skill_invocation)).unwrap()
    );
    drop(admitted);
    converge(&mut kernel).await;
    let (retired, _, context) = capture().await.unwrap();
    assert!(retired.snapshot().names().is_empty());
    assert!(context.contexts.is_empty());
    assert!(matches!(
        first_tools
            .snapshot()
            .prepare(
                "Skill".into(),
                serde_json::json!({"name":"review"}),
                maka_runtime::tools::ToolCallContext {
                    invocation: invocation.clone(),
                    operation_id: "stale".into()
                },
                Default::default()
            )
            .await,
        Err(maka_runtime::tool_call::ToolRejection::Unavailable)
    ));
    let empty = Snapshot::empty();
    assert!(matches!(
        empty.prepare(&mut message("ordinary chat"), &[]).unwrap(),
        InputPreparation::Ready { .. }
    ));
    assert!(matches!(
        empty.prepare(&mut message("/skill:review"), &[]).unwrap(),
        InputPreparation::Blocked(_)
    ));
    composition.roots.get_mut(&Scope::Profile).unwrap()[0].disabled = false;
    kernel.configure(&composition, definitions).unwrap();
    converge(&mut kernel).await;
    let fresh = catalog
        .snapshot::<Skills>(&Scope::Profile)
        .entries
        .remove(ID)
        .unwrap();
    assert_ne!(fresh.owner.identity().unwrap().activation, activation);
    let fresh = fresh
        .value
        .capture(workspace.to_str().unwrap(), Default::default())
        .await
        .unwrap();
    let (reloaded, _, context) = capture().await.unwrap();
    assert_eq!(reloaded.snapshot().names(), ["Skill", "SkillSearch"]);
    assert!(context.contexts[0].contains("Updated review"));
    assert_ne!(
        context.sources[0].activation,
        first_context.sources[0].activation
    );
    let mut input = message("/skill:review");
    fresh.prepare(&mut input, &[]).unwrap();
    assert!(input.text.contains("Replacement instructions."));
    kernel
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}
