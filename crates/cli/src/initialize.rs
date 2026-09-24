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

use crate::args::Root;
use clap::Args;
use maka_config::ConfigurationStore;
use maka_event_log::root::{RootNamespaces, RootOwner, initialize};
use maka_runtime::configuration::policy::{
    NetworkProxy, PrivacyPolicy, RuntimePolicyMutationResult,
    network_update::{CredentialUpdate, Update, UpdateResult},
};
use maka_runtime_host::server::HostError;
use serde::Deserialize;
use std::{
    io::Read,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Args)]
pub(super) struct Init {
    #[command(flatten)]
    root: Root,
    /// Read initial privacy/proxy settings from stdin. Requires a new, empty root.
    #[arg(long)]
    settings_stdin: bool,
}

/// Bootstrap secrets travel through stdin, never command arguments or diagnostics.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    privacy: PrivacyPolicy,
    network_proxy: Option<NetworkProxy>,
    proxy_password: Option<String>,
}

impl Init {
    pub async fn run(self) -> Result<(), HostError> {
        let namespaces = RootNamespaces::for_current_account()?;
        let id = if self.settings_stdin {
            let settings = tokio::task::spawn_blocking(|| {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .lock()
                    .take(65537)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 65536 {
                    return Err(std::io::Error::other("initial settings exceed 64 KiB"));
                }
                serde_json::from_slice::<Settings>(&bytes)
                    .map_err(|_| std::io::Error::other("invalid initial settings"))
            })
            .await??;
            let update = settings.proxy_update()?;
            // Never reconfigure an existing root or race its running Host.
            let owner = Arc::new(RootOwner::create(&self.root.root, &namespaces)?);
            let id = owner.root_id().to_owned();
            let store = ConfigurationStore::for_root(owner).await?;
            if !matches!(
                store.set_privacy(0, settings.privacy).await?,
                RuntimePolicyMutationResult::Committed { revision: 1 }
            ) {
                return Err("initial privacy settings were not committed".into());
            }
            if let Some(update) = update {
                let now = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
                if !matches!(
                    store.update_network_proxy(update, now).await?,
                    UpdateResult::Committed { .. }
                ) {
                    return Err("initial proxy settings were not committed".into());
                }
            }
            store.close().await?;
            id
        } else {
            initialize(&self.root.root, &namespaces)?
        };
        println!("{}", serde_json::json!({"rootId":id}));
        Ok(())
    }
}

impl Settings {
    fn proxy_update(&self) -> Result<Option<Update>, HostError> {
        let Some(proxy) = &self.network_proxy else {
            return if self.proxy_password.is_some() {
                Err("proxy password requires proxy settings".into())
            } else {
                Ok(None)
            };
        };
        let credential = match (proxy.auth_enabled, &self.proxy_password) {
            (true, Some(secret)) => CredentialUpdate::Replace {
                secret: secret.clone(),
                expected_target: None,
            },
            (false, None) => CredentialUpdate::Delete {},
            _ => return Err("proxy authentication does not match its credential".into()),
        };
        let mut update = Update {
            expected_policy_revision: 1,
            expected_credential: None,
            network_proxy: proxy.clone(),
            credential,
        };
        update.normalize()?;
        Ok(Some(update))
    }
}
