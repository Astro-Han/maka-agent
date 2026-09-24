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

use super::{Host, configuration};
use maka_config::model_fetch::ModelFetchPreparation;
use maka_protocol::OperationError;
use maka_runtime::configuration::ConnectionModelFetchResult;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

mod provider;
mod test;
pub(super) use provider::ProviderOperation;

/// Serialize connection effects, not ordinary catalog edits or network-wide traffic.
#[derive(Default)]
pub(super) struct ConnectionEffects {
    lanes: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl ConnectionEffects {
    pub(super) async fn lane(&self, id: &str) -> OwnedMutexGuard<()> {
        let lane = {
            let mut lanes = self.lanes.lock().expect("connection effect lanes poisoned");
            lanes.retain(|_, lane| lane.strong_count() > 0);
            match lanes.get(id).and_then(Weak::upgrade) {
                Some(lane) => lane,
                None => {
                    let lane = Arc::new(AsyncMutex::new(()));
                    lanes.insert(id.to_owned(), Arc::downgrade(&lane));
                    lane
                }
            }
        };
        lane.lock_owned().await
    }

    pub async fn fetch(
        &self,
        host: &Host,
        id: &str,
    ) -> Result<ConnectionModelFetchResult, OperationError> {
        let _lane = self.lane(id).await;
        // Serialize snapshot/binding with committed connection deletion only;
        // no network request or refresh settlement runs under admission.
        let admission = host.executions.lock_admission().await;
        let mut prepared = match host
            .configuration
            .prepare_model_fetch(id)
            .await
            .map_err(configuration::failure)?
        {
            ModelFetchPreparation::Ready(prepared) => prepared,
            ModelFetchPreparation::Rejected(reason) => {
                return Ok(ConnectionModelFetchResult::Rejected { reason });
            }
        };
        let operation = ProviderOperation::prepare(
            host,
            prepared.connection(),
            prepared
                .provider_credential()
                .map_err(configuration::failure)?,
            prepared.network_configuration(),
        )?;
        drop(admission);
        let credential = operation.credential().await?;
        if let Some(credential) = &credential {
            prepared
                .accept_credential(credential.clone())
                .map_err(configuration::failure)?;
        }
        let result = operation
            .discover(credential.as_ref(), prepared.request_headers())
            .await;
        match result {
            Ok(models) => prepared
                .complete(
                    models,
                    configuration::now().map_err(configuration::failure)?,
                )
                .await
                .map_err(configuration::failure),
            Err(error_class) => Ok(ConnectionModelFetchResult::Failed { error_class }),
        }
    }
}
