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
    Error,
    composition::{Composition, Scope},
    contributions::{Catalog, Staged},
    kernel::{Definition, Definitions, Kernel, Plugin, PluginContext},
    services::Services,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::{Instant, timeout};

struct Provider;
impl Plugin for Provider {
    fn validate(&self, _: &Scope, config: &Value) -> Result<(), Error> {
        if config.as_u64().is_none() {
            return Err(Error::Invalid("expected a positive counter".into()));
        }
        Ok(())
    }
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        Box::pin(async move {
            context
                .services
                .provide(
                    &context.lifecycle,
                    "counter",
                    Arc::new(config.as_u64().unwrap()),
                )
                .map_err(|error| error.to_string())?;
            Ok(Staged::default())
        })
    }
}

struct Consumer {
    starts: Arc<AtomicUsize>,
    name: &'static str,
}
impl Plugin for Consumer {
    fn activate(
        &self,
        context: PluginContext,
        _: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let starts = self.starts.clone();
        let name = self.name;
        Box::pin(async move {
            let value = *context
                .services
                .get::<u64>("counter")
                .map_err(|error| error.to_string())?
                .ok_or("dependency disappeared")?
                .acquire()
                .map_err(|error| error.to_string())?;
            context
                .lifecycle
                .spawn("business loop", async move {
                    starts.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .map_err(|error| error.to_string())?;
            let mut staged = Staged::default();
            staged
                .insert(name, value)
                .map_err(|error| error.to_string())?;
            Ok(staged)
        })
    }
}

fn definitions(starts: &Arc<AtomicUsize>, contribution: &'static str) -> Definitions {
    [
        (
            "provider",
            Arc::new(Provider) as Arc<dyn Plugin>,
            Vec::new(),
        ),
        (
            "consumer",
            Arc::new(Consumer {
                starts: starts.clone(),
                name: contribution,
            }) as Arc<dyn Plugin>,
            vec!["counter".into()],
        ),
    ]
    .into_iter()
    .map(|(id, plugin, inject)| {
        (
            id.into(),
            Arc::new(Definition {
                id: id.into(),
                revision: "builtin".into(),
                dependencies: Vec::new(),
                inject,
                plugin,
            }),
        )
    })
    .collect()
}

fn composition(value: Value, disabled: bool) -> Composition {
    Composition {
        roots: BTreeMap::from([(
            Scope::Profile,
            serde_json::from_value(json!([
                {"id":"provider", "packageId":"provider", "config":value, "disabled":disabled},
                {"id":"consumer", "packageId":"consumer"}
            ]))
            .unwrap(),
        )]),
    }
}

async fn converge(kernel: &mut Kernel) {
    timeout(Duration::from_secs(2), async {
        loop {
            if kernel.tick().unwrap().converged {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dependency_replacement_waits_for_cleanup_and_reactivates_consumers() {
    let services = Services::default();
    let catalog = Catalog::default();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut kernel = Kernel::new(services.clone(), catalog.clone());
    kernel
        .configure(&composition(json!(1), false), definitions(&starts, "value"))
        .unwrap();
    converge(&mut kernel).await;
    let old = catalog.snapshot::<u64>(&Scope::Profile);
    let identity = old.entries["value"].owner.identity().unwrap().activation;
    let provider_call = services
        .view()
        .get::<u64>("counter")
        .unwrap()
        .unwrap()
        .acquire()
        .unwrap();
    assert_eq!(*old.entries["value"].value, 1);
    assert!(
        kernel
            .configure(
                &composition(json!("wrong type"), false),
                definitions(&starts, "value")
            )
            .is_err()
    );
    assert!(old.entries["value"].admit().is_ok());

    kernel
        .configure(&composition(json!(2), false), definitions(&starts, "value"))
        .unwrap();
    kernel.tick().unwrap();
    assert!(!kernel.status().cleanup_complete);
    assert!(services.view().get::<u64>("counter").unwrap().is_none());
    assert_eq!(old.entries["value"].admit().err(), Some(Error::Retired));
    drop(provider_call);
    converge(&mut kernel).await;
    let updated = catalog.snapshot::<u64>(&Scope::Profile);
    assert_eq!(*updated.entries["value"].value, 2);
    assert_ne!(
        updated.entries["value"]
            .owner
            .identity()
            .unwrap()
            .activation,
        identity
    );

    kernel
        .configure(&composition(json!(2), true), definitions(&starts, "value"))
        .unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            let status = kernel.tick().unwrap();
            if status.cleanup_complete
                && catalog.snapshot::<u64>(&Scope::Profile).entries.is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    kernel
        .configure(&composition(json!(2), false), definitions(&starts, "value"))
        .unwrap();
    converge(&mut kernel).await;
    kernel
        .shutdown(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}

#[tokio::test]
async fn reserved_publication_only_admits_its_designated_package() {
    for package in [None, Some("provider"), Some("consumer")] {
        let catalog = Catalog::default();
        match package {
            None => catalog.reserve::<u64>("core").unwrap(),
            Some(package) => catalog.reserve_for::<u64>("core", package).unwrap(),
        }
        assert!(catalog.reserve_for::<u64>("core", "impostor").is_err());
        let starts = Arc::new(AtomicUsize::new(0));
        let mut kernel = Kernel::new(Services::default(), catalog.clone());
        kernel
            .configure(&composition(json!(1), false), definitions(&starts, "core"))
            .unwrap();
        if package == Some("consumer") {
            converge(&mut kernel).await;
            let contribution = catalog
                .snapshot::<u64>(&Scope::Profile)
                .entries
                .remove("core")
                .unwrap();
            assert_eq!(*contribution.value, 1);
            let call = contribution.admit().unwrap();
            drop(call);
            kernel
                .shutdown(Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
            assert!(contribution.admit().is_err());
        } else {
            timeout(Duration::from_secs(2), async {
                loop {
                    if kernel
                        .tick()
                        .unwrap()
                        .entries
                        .iter()
                        .any(|entry| entry.error.is_some())
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                starts.load(Ordering::SeqCst),
                0,
                "failed publication cannot start business work"
            );
            assert!(catalog.snapshot::<u64>(&Scope::Profile).entries.is_empty());
            kernel
                .shutdown(Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn recovery_keeps_healthy_entries_running_while_invalid_saved_configuration_is_repaired() {
    use maka_plugins::kernel::Prepared;
    let services = Services::default();
    let catalog = Catalog::default();
    let starts = Arc::new(AtomicUsize::new(0));
    let mut desired = composition(json!(1), false);
    desired.roots.get_mut(&Scope::Profile).unwrap().push(
        serde_json::from_value(json!({
            "id":"incompatible", "packageId":"provider", "config":"old format"
        }))
        .unwrap(),
    );
    let mut kernel = Kernel::new(services, catalog.clone());
    kernel.install(Prepared::recover(&desired, definitions(&starts, "value")).unwrap());
    timeout(Duration::from_secs(2), async {
        loop {
            kernel.tick().unwrap();
            if catalog
                .snapshot::<u64>(&Scope::Profile)
                .entries
                .contains_key("value")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let healthy = catalog.snapshot::<u64>(&Scope::Profile);
    assert!(healthy.entries["value"].admit().is_ok());
    assert!(
        kernel
            .status()
            .entries
            .iter()
            .any(|entry| entry.entry_id == "incompatible" && entry.error.is_some())
    );
    desired.roots.get_mut(&Scope::Profile).unwrap()[1].config = json!({"label":"changed"});
    let prepared = Prepared::recover(&desired, definitions(&starts, "value")).unwrap();
    kernel.validate_change(&prepared).unwrap();
    kernel.install(prepared);
    desired.roots.get_mut(&Scope::Profile).unwrap()[2].disabled = true;
    let prepared = Prepared::recover(&desired, definitions(&starts, "value")).unwrap();
    kernel.validate_change(&prepared).unwrap();
    kernel.install(prepared);
    converge(&mut kernel).await;
    kernel
        .shutdown(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}
