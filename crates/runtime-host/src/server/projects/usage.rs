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

use super::{internal, stored};
use maka_event_log::EventLog;
use maka_protocol::{
    OperationError,
    session::{WorkspaceProjection, WorkspaceTarget},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::broadcast;

/// Project usage and its notification share the Host's existing catalog revision.
#[derive(Clone)]
pub(crate) struct Usage {
    log: Arc<EventLog>,
    changes: broadcast::Sender<Value>,
    revision: Arc<AtomicU64>,
}
impl Usage {
    pub(crate) fn new(
        log: Arc<EventLog>,
        changes: broadcast::Sender<Value>,
        revision: Arc<AtomicU64>,
    ) -> Self {
        Self {
            log,
            changes,
            revision,
        }
    }

    pub(crate) async fn record(
        &self,
        workspace: &WorkspaceProjection,
    ) -> Result<(), OperationError> {
        if let WorkspaceTarget::Project { project_id } = &workspace.target {
            self.log
                .touch_project(
                    project_id,
                    &workspace.host_cwd,
                    super::super::configuration::now().map_err(internal)?,
                )
                .await
                .map_err(stored)?;
            let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self
                .changes
                .send(json!({"kind":"project.catalog.changed", "revision":revision}));
        }
        Ok(())
    }
}
