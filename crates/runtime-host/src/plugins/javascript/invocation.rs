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

mod resources;
use maka_runtime::{event::Invocation, tools::ToolError};
pub(super) use resources::{Resources, Ticket};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

/// Issued by Host dispatch, never reconstructed from plugin-supplied identities.
/// Each module gets its own handle; forwarding retains the original revocation.
#[derive(Clone)]
pub(super) struct Authority {
    pub identity: Identity,
    pub cancellation: CancellationToken,
    pub resources: Arc<Resources>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Identity {
    pub invocation: Invocation,
    pub operation_id: Option<String>,
}
#[derive(Default)]
pub(super) struct Calls(Mutex<BTreeMap<String, Authority>>);
impl Calls {
    pub fn enter(
        self: &Arc<Self>,
        identity: Identity,
        cancellation: CancellationToken,
    ) -> Result<Guard, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Failed("plugin invocation is closed".into()));
        }
        let mut calls = self.0.lock().unwrap();
        if calls.len() >= 128 {
            return Err(ToolError::Failed(
                "plugin invocation capacity exceeded".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let authority = Authority {
            identity,
            cancellation: cancellation.child_token(),
            resources: Arc::default(),
        };
        calls.insert(id.clone(), authority.clone());
        Ok(Guard {
            calls: self.clone(),
            id,
            authority,
        })
    }
    pub fn get(&self, id: &str) -> Result<Authority, maka_plugins::Error> {
        self.0
            .lock()
            .unwrap()
            .get(id)
            .filter(|call| !call.cancellation.is_cancelled())
            .cloned()
            .ok_or(maka_plugins::Error::Retired)
    }
}
pub(super) struct Guard {
    calls: Arc<Calls>,
    pub id: String,
    pub authority: Authority,
}
impl Guard {
    pub async fn finish(&self) -> Result<(), ToolError> {
        self.authority.cancellation.cancel();
        self.authority.resources.finish().await
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.authority.cancellation.cancel();
        self.calls.0.lock().unwrap().remove(&self.id);
    }
}
