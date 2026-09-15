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

mod activation;
mod control;
mod package;
mod service;
mod store;
mod update;
mod updates;
pub(super) use activation::Activate;
pub(super) use control::{Control, ControlAction};
pub(super) use update::Update;

use clap::{Args, ValueEnum};
use maka_event_log::root::{self, FileLease, RootLocation, RootNamespaces, RootOwner};
use maka_runtime_host::server::HostError;
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use uuid::Uuid;

#[derive(Clone)]
struct RootId(String);

impl FromStr for RootId {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.into()))
        } else {
            Err("root ID must contain 64 lowercase hexadecimal characters")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    OnDemand,
    Supervised,
}

#[derive(Args)]
pub(super) struct Install {
    #[command(flatten)]
    root: crate::args::Root,
    #[arg(long, value_enum, default_value = "on-demand")]
    mode: Mode,
    #[arg(long, default_value = "127.0.0.1:0")]
    websocket: SocketAddr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Admission {
    #[default]
    Active,
    Revoked,
}

impl Admission {
    fn is_active(&self) -> bool {
        *self == Self::Active
    }
}

/// Deployment admission authorizes startup; it does not assert process health.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Deployment {
    deployment_id: Uuid,
    config_revision: u64,
    root_id: String,
    root_path: PathBuf,
    pub executable: PathBuf,
    sha256: String,
    pub mode: Mode,
    pub websocket: SocketAddr,
    // Active remains readable by previously installed executables. Older readers
    // reject the explicit tombstone through deny_unknown_fields.
    #[serde(default, skip_serializing_if = "Admission::is_active")]
    admission: Admission,
}

impl Deployment {
    fn require_active(&self) -> Result<(), HostError> {
        if !self.admission.is_active() {
            return Err("deployment has been uninstalled".into());
        }
        Ok(())
    }

    pub fn generation(&self) -> String {
        format!("{}:{}", self.deployment_id, self.config_revision)
    }

    fn validate(&self, root: &RootLocation, directory: &Path) -> Result<(), HostError> {
        if self.root_id != root.root_id()
            || self.root_path != root.canonical_path()
            || !(1..=9_007_199_254_740_991).contains(&self.config_revision)
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.executable != package::path(directory, &self.sha256)
            || self.websocket.ip() != std::net::Ipv4Addr::LOCALHOST
        {
            return Err("deployment does not match its native root or package".into());
        }
        Ok(())
    }
}

pub(super) fn directory(root_id: &str) -> Result<PathBuf, HostError> {
    let namespaces = RootNamespaces::for_current_account()?;
    Ok(namespaces
        .ownership
        .parent()
        .ok_or("missing account data directory")?
        .canonicalize()?
        .join("deployments")
        .join(root_id))
}

impl Install {
    pub async fn run(self) -> Result<(), HostError> {
        if self.websocket.ip() != std::net::Ipv4Addr::LOCALHOST {
            return Err("managed Host listener must use 127.0.0.1".into());
        }
        let root_path = self.root.root;
        let (root, directory, lease) = tokio::task::spawn_blocking(move || {
            root::initialize(&root_path, &RootNamespaces::for_current_account()?)?;
            let root = root::resolve(&root_path)?;
            let directory = directory(root.root_id())?;
            root::private_directory(&directory)?;
            let lease = Arc::new(FileLease::acquire(&directory.join("executor.lock"))?);
            lease.validate()?;
            Ok::<_, HostError>((root, directory, lease))
        })
        .await??;
        let existing = store::read(&directory).await?;
        if let store::Installation::Installed(existing) = &existing
            && existing.admission.is_active()
        {
            existing.validate(&root, &directory)?;
            if existing.mode != self.mode || existing.websocket != self.websocket {
                return Err("deployment configuration changes require an update".into());
            }
            if existing.executable == std::env::current_exe()?.canonicalize()? {
                lease.validate()?;
                println!("{}", serde_json::to_string(existing)?);
                return Ok(());
            }
        }
        let stage_directory = directory.clone();
        let stage_lease = lease.clone();
        let (executable, sha256) = tokio::task::spawn_blocking(move || {
            stage_lease.validate()?;
            let package = package::stage(&stage_directory, &package::source(self.mode)?)?;
            stage_lease.validate()?;
            Ok::<_, HostError>(package)
        })
        .await??;
        let requested = Deployment {
            deployment_id: Uuid::new_v4(),
            config_revision: 1,
            root_id: root.root_id().into(),
            root_path: root.canonical_path().into(),
            executable,
            sha256,
            mode: self.mode,
            websocket: self.websocket,
            admission: Admission::Active,
        };
        let deployment = if let store::Installation::Installed(existing) = existing {
            existing.validate(&root, &directory)?;
            if existing.admission == Admission::Revoked {
                let owner = Arc::new(RootOwner::open(
                    root.canonical_path(),
                    &RootNamespaces::for_current_account()?,
                )?);
                // Remove the old projection before granting a new installation:
                // an old login trigger must not resurrect it across the cut.
                service::remove(existing.clone(), lease.clone(), owner.clone()).await?;
                store::change(
                    &directory,
                    lease.clone(),
                    owner,
                    existing,
                    store::Change::Reinstall(requested),
                )
                .await?
            } else {
                if existing.executable != requested.executable
                    || existing.mode != requested.mode
                    || existing.websocket != requested.websocket
                {
                    return Err(
                        "deployment is already installed; changing it requires an update".into(),
                    );
                }
                existing
            }
        } else {
            // No deployment database is created before obtaining the Root. A busy
            // unmanaged Host remains unmanaged; staging alone cannot fence startup.
            let owner = RootOwner::open(
                root.canonical_path(),
                &RootNamespaces::for_current_account()?,
            )?;
            if owner.root_id() != root.root_id() {
                return Err("State Root changed during installation".into());
            }
            store::install(&directory, lease.clone(), owner, requested).await?
        };
        lease.validate()?;
        println!("{}", serde_json::to_string(&deployment)?);
        Ok(())
    }
}

/// Called with the actual writer lease, before any Host database can migrate.
pub(super) async fn admit(owner: &RootOwner, mode: Mode) -> Result<Option<Deployment>, HostError> {
    let root = root::resolve(owner.canonical_path())?;
    let directory = directory(root.root_id())?;
    let deployment = match store::read(&directory).await? {
        store::Installation::Missing => return Ok(None),
        store::Installation::Incomplete => {
            return Err("deployment installation must finish before Host startup".into());
        }
        store::Installation::Installed(deployment) => deployment,
    };
    deployment.validate(&root, &directory)?;
    deployment.require_active()?;
    if !deployment.executable.symlink_metadata()?.is_file()
        || deployment.executable.canonicalize()? != deployment.executable
        || deployment.mode != mode
        || std::env::current_exe()?.canonicalize()? != deployment.executable
    {
        return Err("this executable or launch mode is not authorized by the deployment".into());
    }
    owner.validate_current()?;
    Ok(Some(deployment))
}
