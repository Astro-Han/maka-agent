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

use maka_runtime::artifact::content_digest;
use maka_skills::publication::{Error, Publisher, Tree};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tokio_util::sync::CancellationToken;

fn tree(body: &str) -> Tree {
    let mut tree = Tree::empty();
    for (path, bytes) in [
        ("SKILL.md", body.as_bytes()),
        ("skill.lock.json", b"lock" as &[u8]),
        (".maka/baseline/SKILL.md", body.as_bytes()),
        ("scripts/check", b"keep this resource"),
    ] {
        tree.insert(path, bytes.to_vec()).unwrap();
    }
    tree
}
#[tokio::test]
async fn publication_replaces_complete_directories_preserves_resources_and_rejects_local_edits() {
    let root = tempfile::tempdir().unwrap();
    let data =
        cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
    let publisher = Publisher::open(&data, &CancellationToken::new()).unwrap();
    assert!(
        matches!(Publisher::open(&data, &CancellationToken::new()),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock),
        "another Entry must not recover or publish the same namespace concurrently"
    );
    let cancellation = CancellationToken::new();
    let original = tree("old instructions");
    publisher
        .publish("review", None, Some(&original), &cancellation)
        .await
        .unwrap();
    let captured = publisher
        .capture("review", &cancellation)
        .await
        .unwrap()
        .unwrap();
    let mut next = captured.clone();
    next.insert("SKILL.md", b"new instructions".to_vec())
        .unwrap();
    next.insert(".maka/baseline/SKILL.md", b"new instructions".to_vec())
        .unwrap();
    std::fs::write(root.path().join("skills/review/SKILL.md"), "local edit").unwrap();
    assert!(matches!(
        publisher
            .publish("review", Some(&captured), Some(&next), &cancellation)
            .await,
        Err(Error::Conflict)
    ));
    assert_eq!(
        std::fs::read(root.path().join("skills/review/SKILL.md")).unwrap(),
        b"local edit"
    );
    let captured = publisher
        .capture("review", &cancellation)
        .await
        .unwrap()
        .unwrap();
    publisher
        .publish("review", Some(&captured), Some(&next), &cancellation)
        .await
        .unwrap();
    let updated = publisher
        .capture("review", &cancellation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.get("SKILL.md"),
        Some(b"new instructions".as_slice())
    );
    assert_eq!(
        updated.get(".maka/baseline/SKILL.md"),
        updated.get("SKILL.md")
    );
    assert_eq!(
        updated.get("scripts/check"),
        Some(b"keep this resource".as_slice())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = root.path().join("skills/review/scripts/check");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o751)).unwrap();
        let captured = publisher
            .capture("review", &cancellation)
            .await
            .unwrap()
            .unwrap();
        let mut next = captured.clone();
        next.insert("SKILL.md", b"another update".to_vec()).unwrap();
        publisher
            .publish("review", Some(&captured), Some(&next), &cancellation)
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o751
        );
    }
    let captured = publisher
        .capture("review", &cancellation)
        .await
        .unwrap()
        .unwrap();
    publisher
        .publish("review", Some(&captured), None, &cancellation)
        .await
        .unwrap();
    assert!(
        publisher
            .capture("review", &cancellation)
            .await
            .unwrap()
            .is_none()
    );
    publisher.recover().await.unwrap();
    assert_eq!(
        std::fs::read_dir(root.path().join("transactions"))
            .unwrap()
            .count(),
        0
    );
}

fn manifest(body: &str) -> Value {
    json!({
        ".maka":{"kind":"directory"}, ".maka/baseline":{"kind":"directory"},
        "SKILL.md":{"kind":"file","hash":content_digest(body.as_bytes()),"mode":384},
        ".maka/baseline/SKILL.md":{"kind":"file","hash":content_digest(body.as_bytes()),"mode":384},
        "skill.lock.json":{"kind":"file","hash":content_digest(b"lock"),"mode":384},
        "scripts":{"kind":"directory"},
        "scripts/check":{"kind":"file","hash":content_digest(b"keep this resource"),"mode":384}
    })
}
fn write_tree(path: &Path, body: &str) {
    let files = BTreeMap::from([
        ("SKILL.md", body.as_bytes()),
        ("skill.lock.json", b"lock".as_slice()),
        (".maka/baseline/SKILL.md", body.as_bytes()),
        ("scripts/check", b"keep this resource".as_slice()),
    ]);
    for (name, bytes) in files {
        let path = path.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
}
#[tokio::test]
async fn recovery_finishes_each_publication_cut_and_preserves_edits_after_commit() {
    for cut in [
        "intent",
        "old_moved",
        "published",
        "committed",
        "conflict",
        "lost_intent",
    ] {
        let root = tempfile::tempdir().unwrap();
        let data =
            cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
        let publisher = Publisher::open(&data, &CancellationToken::new()).unwrap();
        let target = root.path().join("skills/review");
        write_tree(&target, "old");
        let intent = serde_json::to_vec(&json!({
            "schema":1,"id":"review","expected":manifest("old"),"next":manifest("new")
        }))
        .unwrap();
        let hash = content_digest(&intent);
        let transactions = root.path().join("transactions");
        let transaction = transactions.join(format!("tx-{}-{}", uuid::Uuid::new_v4(), &hash[7..]));
        write_tree(&transaction.join("next"), "new");
        std::fs::write(transaction.join("intent.json"), &intent).unwrap();
        if matches!(cut, "old_moved" | "published" | "committed" | "lost_intent") {
            std::fs::rename(&target, transaction.join("old")).unwrap();
        }
        if matches!(cut, "published" | "committed") {
            std::fs::rename(transaction.join("next"), &target).unwrap();
        }
        if cut == "lost_intent" {
            std::fs::remove_file(transaction.join("intent.json")).unwrap();
            assert!(publisher.recover().await.is_err());
            assert_eq!(
                std::fs::read(transaction.join("old/SKILL.md")).unwrap(),
                b"old"
            );
            continue;
        }
        if cut == "committed" {
            std::fs::write(transaction.join("committed"), &hash).unwrap();
            std::fs::write(target.join("SKILL.md"), "edit after commit").unwrap();
        } else if cut == "conflict" {
            std::fs::write(target.join("SKILL.md"), "edit before takeover").unwrap();
        }
        publisher.recover().await.unwrap();
        let expected = match cut {
            "committed" => "edit after commit",
            "conflict" => "edit before takeover",
            _ => "new",
        };
        assert_eq!(
            std::fs::read(target.join("SKILL.md")).unwrap(),
            expected.as_bytes(),
            "{cut}"
        );
        assert_eq!(
            std::fs::read(target.join("scripts/check")).unwrap(),
            b"keep this resource"
        );
        assert_eq!(std::fs::read_dir(transactions).unwrap().count(), 0);
        publisher.recover().await.unwrap();
    }
}
