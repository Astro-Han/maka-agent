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

use maka_plugins::{
    Error,
    composition::{Entry, Scope},
    contributions::{Catalog, Staged},
    fiber::Fiber,
    services::Services,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

#[tokio::test]
async fn services_publish_before_business_and_snapshots_reject_retired_starts() {
    let services = Services::default();
    let view = services.view();
    let catalog = Catalog::default();
    catalog.reserve::<String>("core").unwrap();
    let provider = Fiber::new("provider", "provider", Scope::Profile).unwrap();
    provider.begin_loading().unwrap();
    view.provide(&provider.context(), "clock", Arc::new(42_u32))
        .unwrap();
    assert!(view.get::<u32>("clock").unwrap().is_none());
    provider.ready().unwrap();
    let old_service = view.get::<u32>("clock").unwrap().unwrap();
    assert_eq!(*old_service.acquire().unwrap(), 42);
    assert_eq!(provider.context().admit().err(), Some(Error::Retired));

    let mut rejected = Staged::default();
    rejected
        .insert("partial", "not visible".to_owned())
        .unwrap();
    rejected
        .insert("core", "cannot override".to_owned())
        .unwrap();
    assert!(matches!(
        catalog.publish(&provider, rejected),
        Err(Error::ContributionConflict(_))
    ));
    assert!(
        catalog
            .snapshot::<String>(&Scope::Profile)
            .entries
            .is_empty()
    );
    assert!(!provider.context().is_effective());

    let mut staged = Staged::default();
    staged.insert("answer", "profile".to_owned()).unwrap();
    catalog.publish(&provider, staged).unwrap();
    let original = catalog.snapshot::<String>(&Scope::Session("session".into()));
    let call = original.entries["answer"].admit().unwrap();
    let local = Fiber::new("local", "local", Scope::Session("session".into())).unwrap();
    local.begin_loading().unwrap();
    local.ready().unwrap();
    let mut staged = Staged::default();
    staged.insert("answer", "session".to_owned()).unwrap();
    catalog.publish(&local, staged).unwrap();
    assert_eq!(
        &**catalog
            .snapshot::<String>(&Scope::Session("session".into()))
            .entries["answer"]
            .value,
        "session"
    );
    assert_eq!(&**original.entries["answer"].value, "profile");

    provider.retire();
    assert_eq!(
        original.entries["answer"].admit().err(),
        Some(Error::Retired)
    );
    assert_eq!(
        old_service.acquire().err().map(|e| e.to_string()),
        Some(Error::Retired.to_string())
    );
    assert!(view.get::<u32>("clock").unwrap().is_none());
    let replacement = Fiber::new("provider", "provider", Scope::Profile).unwrap();
    replacement.begin_loading().unwrap();
    view.provide(&replacement.context(), "clock", Arc::new(7_u32))
        .unwrap();
    replacement.ready().unwrap();
    assert_eq!(
        *view
            .get::<u32>("clock")
            .unwrap()
            .unwrap()
            .acquire()
            .unwrap(),
        7
    );
    assert!(old_service.acquire().is_err());
    drop(call);
    let deadline = Instant::now() + Duration::from_secs(1);
    provider.shutdown(deadline).await.unwrap();
    local.shutdown(deadline).await.unwrap();
    replacement.shutdown(deadline).await.unwrap();
}

#[tokio::test]
async fn registration_retirement_is_independent_of_instance_and_keeps_admitted_calls() {
    let catalog = Catalog::default();
    let services = Services::default().view();
    let owner = Fiber::new("example", "example", Scope::Profile).unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    owner.publish().unwrap();
    let context = owner.context();
    for generation in 0..64 {
        let service_registration = services
            .register(&context, "dynamic", Arc::new(generation))
            .unwrap();
        let original_service = services.get::<i32>("dynamic").unwrap().unwrap();
        let service_call = original_service.acquire().unwrap();
        drop(service_registration);
        assert!(services.get::<i32>("dynamic").unwrap().is_none());
        assert!(matches!(original_service.acquire(), Err(Error::Retired)));
        assert_eq!(*service_call, generation);
        drop(service_call);
        let mut staged = Staged::default();
        staged.insert("dynamic", generation).unwrap();
        let registration = catalog.register(&context, staged).unwrap();
        let captured = catalog.snapshot::<i32>(&Scope::Profile);
        let original = &captured.entries["dynamic"];
        let admitted = original.admit().unwrap();
        let mut conflict = Staged::default();
        conflict.insert("dynamic", -1_i32).unwrap();
        assert!(matches!(
            catalog.register(&context, conflict),
            Err(Error::ContributionConflict(_))
        ));
        drop(registration);
        assert!(context.is_effective());
        assert!(catalog.snapshot::<i32>(&Scope::Profile).entries.is_empty());
        assert!(matches!(original.admit(), Err(Error::Retired)));
        assert_eq!(*original.value, generation);
        assert_eq!(context.active_calls(), 1);
        drop(admitted);
        assert_eq!(context.active_calls(), 0);
    }
    owner
        .shutdown(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn inherited_isolation_and_intercepts_do_not_create_resource_owners() {
    let services = Services::default();
    let root = services.view();
    let spec: Entry = serde_json::from_value(serde_json::json!({
        "id": "subtree", "isolate": {"store": true}, "intercept": {"store": {"readOnly": true}}
    }))
    .unwrap();
    let isolated = root.derive(&spec).unwrap();
    let child = isolated
        .derive(
            &serde_json::from_value(serde_json::json!({
                "id": "child", "intercept": {"store": {"prefix": "child"}}
            }))
            .unwrap(),
        )
        .unwrap();
    let owner = Fiber::new("storage", "storage", Scope::Profile).unwrap();
    owner.begin_loading().unwrap();
    isolated
        .provide(&owner.context(), "store", Arc::new("private".to_owned()))
        .unwrap();
    owner.ready().unwrap();
    assert!(root.get::<String>("store").unwrap().is_none());
    assert_eq!(
        &**child
            .get::<String>("store")
            .unwrap()
            .unwrap()
            .acquire()
            .unwrap(),
        "private"
    );
    assert_eq!(child.intercepts("store").len(), 2);
    assert!(matches!(
        child.get::<u32>("store"),
        Err(Error::ServiceType(_))
    ));
    owner
        .shutdown(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(child.get::<String>("store").unwrap().is_none());
}
