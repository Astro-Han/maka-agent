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

use futures_util::{future::BoxFuture, poll};
use maka_plugins::{
    composition::Scope,
    contributions::{Catalog, Staged},
    fiber::Fiber,
    input::*,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

struct Example(Revision);
impl Provider for Example {
    fn prepare(
        &self,
        mut request: Request,
    ) -> BoxFuture<'static, Result<Outcome, maka_plugins::Error>> {
        let revision = self.0.clone();
        Box::pin(async move {
            let basis = revision.capture().await;
            request.content.text.push_str("\nprepared business input");
            Ok(Outcome::Ready {
                content: request.content,
                receipt: json!({"ticket":42}),
                required_tools: Default::default(),
                basis: Some(basis),
            })
        })
    }
}

#[tokio::test]
async fn admission_orders_domain_invalidation_and_retirement_without_executing_callbacks() {
    let catalog = Catalog::default();
    let owner = Fiber::new("example", "example", Scope::Profile).unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    let revision = Revision::default();
    let mut staged = Staged::default();
    staged
        .insert(
            "example.prepare",
            InputPreparation(Arc::new(Example(revision.clone()))),
        )
        .unwrap();
    catalog.publish(&owner, staged).unwrap();
    let request = Request {
        session_id: "session".into(),
        cwd: ".".into(),
        content: "original".into(),
        selections: Default::default(),
        tools: Default::default(),
        cancellation: Default::default(),
    };
    let first = prepare(&catalog, &Scope::Session("session".into()), request.clone())
        .await
        .unwrap();
    assert_eq!(first.content.preparation[0].source.package_id, "example");
    let accepted = serde_json::to_vec(&first.content).unwrap();
    let admission = first.admit().unwrap().unwrap();
    let invalidation = revision.invalidate();
    tokio::pin!(invalidation);
    assert!(
        poll!(&mut invalidation).is_pending(),
        "a domain writer waits for durable input admission"
    );
    drop(admission);
    let writer = invalidation.await;
    assert!(
        first.admit().unwrap().is_none(),
        "commit never waits for an in-progress domain writer"
    );
    drop(writer);
    assert!(
        first.admit().unwrap().is_none(),
        "stale preparation must be repeated"
    );
    let fresh = prepare(&catalog, &Scope::Session("session".into()), request)
        .await
        .unwrap();
    let admitted = fresh.admit().unwrap().unwrap();
    let stopping = owner.shutdown(tokio::time::Instant::now() + Duration::from_secs(1));
    tokio::pin!(stopping);
    assert!(poll!(&mut stopping).is_pending());
    assert!(fresh.admit().is_err(), "retirement fences a new start");
    assert_eq!(
        accepted,
        serde_json::to_vec(&first.content).unwrap(),
        "accepted evidence survives lifecycle changes"
    );
    drop(admitted);
    stopping.await.unwrap();
}
