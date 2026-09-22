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
    composition::Scope,
    contributions::{Catalog, Staged},
    fiber::Fiber,
    model::Credentials,
    provider::{
        Binding, Connection, Definition, Descriptor, Error, Model, Provider, Resolve,
        authentication::Credential,
    },
};
use serde_json::json;
use std::sync::Arc;

struct Fixture;
impl Provider for Fixture {
    fn resolve(&self, _: Resolve) -> BoxFuture<'_, Result<Model, Error>> {
        Box::pin(async { Err(Error::Unavailable) })
    }
    fn authorize(
        &self,
        _: Connection,
        _: Option<Credential>,
        _: String,
    ) -> BoxFuture<'_, Result<Credentials, Error>> {
        Box::pin(async { Err(Error::AuthenticationRequired) })
    }
}
fn publish(catalog: &Catalog, package: &str, scope: Scope) -> (Fiber, maka_plugins::Registration) {
    let owner = Fiber::new(package, package, scope).unwrap();
    owner.begin_loading().unwrap();
    owner.ready().unwrap();
    owner.publish().unwrap();
    let mut staged = Staged::default();
    staged
        .insert(
            "models",
            Definition::new(
                Descriptor {
                    label: package.into(),
                    configuration_schema: json!({"type":"object"}),
                    configuration_defaults: json!({}),
                    authentication: vec![],
                    discovery: false,
                },
                Arc::new(Fixture),
            )
            .unwrap(),
        )
        .unwrap();
    let registration = catalog.register(&owner.context(), staged).unwrap();
    (owner, registration)
}

#[tokio::test]
async fn persisted_recipient_survives_reload_but_never_rebinds_to_a_shadow_or_impostor() {
    let catalog = Catalog::default();
    let (owner, registration) = publish(&catalog, "example.account", Scope::Profile);
    let binding = Binding::new(
        "models".into(),
        catalog
            .snapshot(&Scope::Profile)
            .entries
            .remove("models")
            .unwrap(),
    )
    .unwrap();
    let recipient = binding.identity().clone();
    let (_shadow, _shadow_registration) =
        publish(&catalog, "example.shadow", Scope::Session("s".into()));
    assert_eq!(
        Binding::resolve(&recipient, &catalog).unwrap().identity(),
        &recipient
    );

    let admitted = binding.admit().unwrap();
    drop(registration);
    assert!(binding.admit().is_err());
    assert!(matches!(
        Binding::resolve(&recipient, &catalog),
        Err(Error::Unavailable)
    ));
    // An already admitted call owns its guard independently of publication.
    drop(admitted);
    drop(owner);

    let (impostor, impostor_registration) = publish(&catalog, "example.other", Scope::Profile);
    assert!(matches!(
        Binding::resolve(&recipient, &catalog),
        Err(Error::Unavailable)
    ));
    drop(impostor_registration);
    drop(impostor);

    let (_restored, _restored_registration) = publish(&catalog, "example.account", Scope::Profile);
    assert!(
        Binding::resolve(&recipient, &catalog)
            .unwrap()
            .admit()
            .is_ok()
    );
    assert!(binding.admit().is_err());
}
