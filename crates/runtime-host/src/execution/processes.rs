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

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default, Clone)]
pub(super) struct Registry(Arc<Mutex<HashMap<String, HashMap<Uuid, Control>>>>);

struct Control {
    stop: CancellationToken,
    completed: CancellationToken,
}

impl Registry {
    pub(super) fn has(&self, session: &str) -> bool {
        self.0.lock().unwrap().contains_key(session)
    }

    /// The caller fences admission before requesting stop. Completion means
    /// the resource owner has released native handles, not merely cancelled them.
    pub(super) fn stop(&self, session: &str) -> Vec<CancellationToken> {
        self.0
            .lock()
            .unwrap()
            .get(session)
            .into_iter()
            .flat_map(|entries| entries.values())
            .map(|entry| {
                entry.stop.cancel();
                entry.completed.clone()
            })
            .collect()
    }
}

impl super::Executions {
    pub(crate) fn own_plugin_process(
        &self,
        session: &str,
        stop: CancellationToken,
    ) -> ProcessActivity {
        let id = Uuid::new_v4();
        let completed = CancellationToken::new();
        self.plugin_processes
            .0
            .lock()
            .unwrap()
            .entry(session.into())
            .or_default()
            .insert(
                id,
                Control {
                    stop,
                    completed: completed.clone(),
                },
            );
        ProcessActivity {
            log: self.log.clone(),
            registry: self.plugin_processes.clone(),
            session: session.into(),
            id,
            completed,
            shutdown: self.shutdown.clone(),
            cleanup_confirmed: true,
        }
    }
}

/// Instance-lifetime processes outlive Turns but retain Session resource ownership.
pub(crate) struct ProcessActivity {
    log: Arc<maka_event_log::EventLog>,
    registry: Registry,
    session: String,
    id: Uuid,
    completed: CancellationToken,
    shutdown: CancellationToken,
    cleanup_confirmed: bool,
}

impl ProcessActivity {
    pub(crate) async fn start(&mut self) -> Result<(), maka_event_log::StoreError> {
        self.cleanup_confirmed = false;
        self.log.admit_session_process(&self.session, self.id).await
    }
    pub(crate) async fn settle(mut self, clean: bool) -> Result<(), maka_event_log::StoreError> {
        if clean {
            self.log.clean_session_process(self.id).await?;
        }
        self.cleanup_confirmed = clean;
        Ok(())
    }
}

impl Drop for ProcessActivity {
    fn drop(&mut self) {
        // A dropped worker is not evidence of cleanup. Before start, however,
        // publication failure cannot have created an OS resource.
        if !self.cleanup_confirmed {
            self.shutdown.cancel();
        }
        let mut registry = self.registry.0.lock().unwrap();
        let entries = registry.get_mut(&self.session).expect("registered process");
        entries.remove(&self.id);
        if entries.is_empty() {
            registry.remove(&self.session);
        }
        self.completed.cancel();
    }
}
