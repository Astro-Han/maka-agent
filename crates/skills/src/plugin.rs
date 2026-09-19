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

//! Statically linked Skills domain; Host supplies only its preferences repository and paths.
use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::Scope,
    contributions::Staged,
    fiber::Context,
    kernel::{Plugin, PluginContext},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

mod catalog;
mod files;
mod import;
mod input;
mod mutation;
mod page;
mod path;
mod preview;
pub mod remote;
mod snapshot;
mod tools;
pub use snapshot::{InputPreparation, Snapshot};

pub const ID: &str = "maka.skills";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Skills plugin is not active")]
    Retired,
    #[error("Skill update may have committed: {0}")]
    OutcomeUnknown(String),
    #[error("Skill source is unavailable: {0}")]
    Source(String),
    #[error("Prepared skill content exceeds durable admission limits")]
    InputTooLarge,
    #[error("Invalid Skill preparation: {0}")]
    Invalid(String),
    #[error("Skill projection failed: {0}")]
    Projection(String),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error(transparent)]
    Worker(#[from] tokio::task::JoinError),
}

pub struct PreferenceSnapshot {
    pub revision: u64,
    pub entries: BTreeMap<String, crate::Preference>,
}

/// Domain repository, not a general configuration or Host handle.
pub trait PreferenceStore: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<PreferenceSnapshot, String>>;
    /// False means the expected control revision no longer matches.
    fn compare_exchange(
        &self,
        expected_revision: u64,
        reference: String,
        preference: crate::Preference,
    ) -> BoxFuture<'_, Result<bool, String>>;
}

pub struct Builtin {
    pub state_root: PathBuf,
    pub home: Option<PathBuf>,
    pub preferences: Arc<dyn PreferenceStore>,
    pub client: Option<remote::ClientSupport>,
}

#[derive(Clone)]
pub struct Skills {
    state_root: PathBuf,
    data: maka_plugins::storage::Directory,
    home: Option<PathBuf>,
    basis: Basis,
    mutations: Arc<tokio::sync::RwLock<()>>,
    input_revision: maka_plugins::input::Revision,
    changed: tokio::sync::watch::Sender<()>,
}

#[derive(Clone)]
struct Basis {
    owner: Context,
    preferences: Arc<dyn PreferenceStore>,
}

impl Plugin for Builtin {
    fn supports_scope(&self, scope: &Scope) -> bool {
        *scope == Scope::Profile || (*scope == Scope::DesktopUi && self.client.is_some())
    }

    fn validate(&self, _: &Scope, config: &Value) -> Result<(), maka_plugins::Error> {
        if config.is_null() || config.as_object().is_some_and(|object| object.is_empty()) {
            Ok(())
        } else {
            Err(maka_plugins::Error::Invalid(
                "Skills takes no instance configuration".into(),
            ))
        }
    }

    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        if context
            .lifecycle
            .identity()
            .is_ok_and(|identity| identity.scope == Scope::DesktopUi)
        {
            return Box::pin(async move {
                let provider = context
                    .services
                    .get::<remote::ClientSupport>(remote::CLIENT_SERVICE)
                    .map_err(|error| error.to_string())?
                    .ok_or("Skills backend is not active")?;
                let client = provider.acquire().map_err(|error| error.to_string())?;
                let identity = context
                    .lifecycle
                    .identity()
                    .map_err(|error| error.to_string())?;
                let mut staged = Staged::default();
                staged
                    .insert(
                        identity.entry_id,
                        maka_plugins::client::Client {
                            bundle: client.bundle.clone(),
                            config,
                        },
                    )
                    .map_err(|error| error.to_string())?;
                Ok(staged)
            });
        }
        let client = self.client.clone();
        let services = context.services.clone();
        let Some(data) = context.data else {
            return Box::pin(async { Err("Skills requires plugin private files".into()) });
        };
        let skills = Skills {
            data,
            mutations: Arc::default(),
            input_revision: Default::default(),
            changed: tokio::sync::watch::channel(()).0,
            state_root: self.state_root.clone(),
            home: self.home.clone(),
            basis: Basis {
                owner: context.lifecycle,
                preferences: self.preferences.clone(),
            },
        };
        Box::pin(async move {
            let root = skills.state_root.clone();
            let home = skills.home.clone();
            skills
                .data
                .run(move |data| {
                    let publisher = crate::publication::Publisher::open(&root, data)?;
                    publisher.recover_legacy(&root)?;
                    publisher.recover()?;
                    if let Some(home) = home {
                        crate::publication::UserStore::recover_existing(&home)?;
                    }
                    Ok::<_, crate::publication::Error>(())
                })
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
            let mut staged = Staged::default();
            tools::publish(&skills, &mut staged)?;
            if let Some(client) = client {
                remote::publish(&skills, &mut staged, client.clone())?;
                services
                    .provide(
                        &skills.basis.owner,
                        remote::CLIENT_SERVICE,
                        Arc::new(client),
                    )
                    .map_err(|error| error.to_string())?;
            }
            staged
                .insert(
                    ID,
                    maka_plugins::input::InputPreparation(Arc::new(skills.clone())),
                )
                .map_err(|error| error.to_string())?;
            staged
                .insert(ID, skills)
                .map_err(|error| error.to_string())?;
            Ok(staged)
        })
    }
}

impl Skills {
    fn notify_on_exit(&self) -> ChangeNotice {
        ChangeNotice(self.changed.clone())
    }
    pub async fn capture(&self, cwd: &str, tools: HashSet<String>) -> Result<Snapshot, Error> {
        let _call = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let view = self.mutations.clone().read_owned().await;
        let input_basis = Some(self.input_revision.capture().await);
        let (preference_revision, preferences) = match self.basis.preferences.read().await {
            Ok(snapshot) => (
                Some(snapshot.revision),
                crate::Preferences::Available(snapshot.entries),
            ),
            Err(_) => (None, crate::Preferences::Unavailable),
        };
        let sources =
            crate::Source::standard(Path::new(cwd), &self.state_root, self.home.as_deref());
        let cancellation = self.basis.owner.stopping().map_err(|_| Error::Retired)?;
        let discovery = tokio::task::spawn_blocking(move || {
            let (_call, _view) = (_call, view);
            crate::scan(&sources, &cancellation)
        })
        .await?
        .map_err(|error| Error::Source(error.to_string()))?;
        if !self.basis.owner.is_effective() {
            return Err(Error::Retired);
        }
        Ok(Snapshot {
            input_basis,
            preference_revision,
            discovery,
            preferences,
            host: crate::HostCapabilities {
                tools,
                capabilities: Default::default(),
            },
            basis: Some(self.basis.owner.clone()),
        })
    }

    async fn governance(
        &self,
        cwd: &str,
    ) -> Result<(crate::SourceCatalog, Option<PreferenceSnapshot>), Error> {
        let _call = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let preferences = self.basis.preferences.read().await.ok();
        let cwd = PathBuf::from(cwd);
        let root = self.state_root.clone();
        let home = self.home.clone();
        let cancellation = self.basis.owner.stopping().map_err(|_| Error::Retired)?;
        let result = tokio::task::spawn_blocking(move || {
            let _call = _call;
            crate::governance_catalog(&cwd, &root, home.as_deref(), &cancellation)
        })
        .await?
        .map_err(|error| Error::Source(error.to_string()))?;
        if !self.basis.owner.is_effective() {
            return Err(Error::Retired);
        }
        Ok((result, preferences))
    }
}

/// An invalidation hint, not another catalog revision or persistence authority.
struct ChangeNotice(tokio::sync::watch::Sender<()>);
impl Drop for ChangeNotice {
    fn drop(&mut self) {
        self.0.send_replace(());
    }
}
