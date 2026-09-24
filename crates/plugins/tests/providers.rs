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
    model::{Connect, Credentials, Socket, Transport},
    provider::{
        Binding, Connection, Context, Definition, Descriptor, Error, Model, Provider, Resolve,
        authentication::{Authenticate, Credential, Method},
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
    fn refresh(
        &self,
        _: Connection,
        mut credential: Credential,
        _: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async move {
            credential.secret = "rotated".into();
            Ok(credential)
        })
    }
    fn authenticate(
        &self,
        _: Authenticate,
        _: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async {
            Ok(Credential {
                secret: "authenticated".into(),
                refresh_at: None,
            })
        })
    }
}

impl Transport for Fixture {
    fn identity(&self) -> u64 {
        1
    }
    fn request(
        &self,
        _: maka_plugins::http::Request,
    ) -> BoxFuture<'_, Result<maka_plugins::http::Response, maka_plugins::model::Error>> {
        Box::pin(async { panic!("unexpected HTTP") })
    }
    fn connect(
        &self,
        _: Connect,
    ) -> BoxFuture<'_, Result<Arc<dyn Socket>, maka_plugins::model::Error>> {
        Box::pin(async { panic!("unexpected WebSocket") })
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
                    authentication: vec![Method {
                        id: "login".into(),
                        label: "Login".into(),
                        input_schema: json!({"type":"object"}),
                        interactive: false,
                    }],
                    anonymous: false,
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

    let admitted = binding
        .prepare_refresh(
            Connection {
                id: "connection".into(),
                revision: 1,
                configuration: json!({}),
            },
            Credential {
                secret: "original".into(),
                refresh_at: Some(1),
            },
        )
        .unwrap();
    let login = binding
        .prepare_authenticate(
            Authenticate {
                connection: Connection {
                    id: "connection".into(),
                    revision: 1,
                    configuration: json!({}),
                },
                method: "login".into(),
                input: json!({}),
            },
            Context {
                transport: Arc::new(Fixture),
                cancellation: tokio_util::sync::CancellationToken::new(),
                interaction: None,
            },
        )
        .unwrap();
    drop(registration);
    assert!(binding.admit().is_err());
    assert!(matches!(
        Binding::resolve(&recipient, &catalog),
        Err(Error::Unavailable)
    ));
    // Host can persist its grant claim between admission and execution. A
    // retirement in that interval must not prevent the admitted callback.
    let credential = admitted
        .run(Context {
            transport: Arc::new(Fixture),
            cancellation: tokio_util::sync::CancellationToken::new(),
            interaction: None,
        })
        .await
        .unwrap();
    assert_eq!(credential.secret, "rotated");
    assert_eq!(login.run().await.unwrap().secret, "authenticated");
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
