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

pub(super) use maka_plugins::call::Scope as Authority;
use maka_runtime::tools::ToolError;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub(super) struct Calls {
    issuer: maka_plugins::call::Issuer,
    invocations: Mutex<BTreeMap<String, Authority>>,
    remotes: Mutex<BTreeMap<String, maka_plugins::remote::Caller>>,
}
impl Calls {
    pub fn new(issuer: maka_plugins::call::Issuer) -> Self {
        Self {
            issuer,
            invocations: Default::default(),
            remotes: Default::default(),
        }
    }
    pub fn forward(self: &Arc<Self>, authority: Authority) -> Result<Guard, ToolError> {
        if !self.issuer.owns(&authority) || authority.cancellation.is_cancelled() {
            return Err(ToolError::Failed(
                "foreign or closed plugin invocation".into(),
            ));
        }
        let mut calls = self.invocations.lock().unwrap();
        if calls.len() >= 128 {
            return Err(ToolError::Failed(
                "plugin invocation capacity exceeded".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        calls.insert(id.clone(), authority.clone());
        Ok(Guard {
            calls: self.clone(),
            id,
            authority,
        })
    }
    pub fn get(&self, id: &str) -> Result<Authority, maka_plugins::Error> {
        self.invocations
            .lock()
            .unwrap()
            .get(id)
            .filter(|call| !call.cancellation.is_cancelled())
            .cloned()
            .ok_or(maka_plugins::Error::Retired)
    }

    pub fn enter_remote(
        self: &Arc<Self>,
        mut caller: maka_plugins::remote::Caller,
    ) -> Result<RemoteGuard, maka_plugins::remote::Error> {
        if caller.cancellation.is_cancelled() {
            return Err(maka_plugins::remote::Error::Cancelled);
        }
        let mut calls = self.remotes.lock().unwrap();
        if calls.len() >= 128 {
            return Err(maka_plugins::remote::Error::Invalid(
                "Remote call capacity exceeded".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        caller.cancellation = caller.cancellation.child_token();
        let cancellation = caller.cancellation.clone();
        calls.insert(id.clone(), caller);
        Ok(RemoteGuard {
            calls: self.clone(),
            id,
            cancellation,
        })
    }

    pub fn remote(&self, id: &str) -> Result<maka_plugins::remote::Caller, maka_plugins::Error> {
        self.remotes
            .lock()
            .unwrap()
            .get(id)
            .filter(|caller| !caller.cancellation.is_cancelled())
            .cloned()
            .ok_or(maka_plugins::Error::Retired)
    }
}
pub(super) struct RemoteGuard {
    calls: Arc<Calls>,
    pub id: String,
    pub cancellation: CancellationToken,
}
impl Drop for RemoteGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.calls.remotes.lock().unwrap().remove(&self.id);
    }
}
pub(super) struct Guard {
    calls: Arc<Calls>,
    pub id: String,
    pub authority: Authority,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.authority.cancellation.cancel();
        self.calls.invocations.lock().unwrap().remove(&self.id);
    }
}
