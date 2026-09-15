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

use super::{ConnectionCount, Host, HostError, LifecycleMode, authority::Authority};
use maka_protocol::{
    COMPATIBILITY_EPOCH, COMPOSITION_ID,
    handshake::{ClientHello, HostHandshake, Lifecycle, ProtocolRange, Replacement},
};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use tokio_util::sync::DropGuard;
use uuid::Uuid;

type Admission<'a> = (
    HostHandshake,
    Option<ConnectionCount<'a>>,
    Option<DropGuard>,
);

impl Host {
    /// Reserve accepted connection residency under the same gate as idle takeover.
    pub(super) fn admit_handshake(
        &self,
        hello: &ClientHello,
        authority: &Authority,
        connection_id: Uuid,
    ) -> Result<Admission<'_>, HostError> {
        let mut retiring = self.retirement.lock().unwrap_or_else(|e| e.into_inner());
        let draining = || HostHandshake::Draining {
            host_epoch: self.epoch.clone(),
            composition_id: COMPOSITION_ID.into(),
            composition_revision: "3".into(),
        };
        if *retiring != super::retirement::Phase::Ready || self.draining.is_cancelled() {
            return Ok((draining(), None, None));
        }
        let ephemeral = self.options.lifecycle_mode == LifecycleMode::Ephemeral;
        let generation_mismatch =
            ephemeral && hello.generation.is_some() && hello.generation != self.options.generation;
        let local_owner = matches!(authority, Authority::LocalOwner);
        let settled = self.accepted_connections.load(Ordering::SeqCst) == 0
            && self.requests.is_empty()
            && self.executions.active_count() == 0
            && self.shells.active_count() == 0;
        if generation_mismatch
            && local_owner
            && hello
                .takeover
                .as_ref()
                .is_some_and(|takeover| takeover.expected_host_epoch == self.epoch)
            && settled
            && self.connections.load(Ordering::SeqCst) == 1
        {
            *retiring = super::retirement::Phase::Retiring;
            // Close admission now, but let the owner flush its draining reply before
            // stopping the listener. Errors/drop still finish the reserved retirement.
            return Ok((draining(), None, Some(self.draining.clone().drop_guard())));
        }
        let selected = hello
            .negotiate(
                ProtocolRange { min: 0, max: 0 },
                COMPATIBILITY_EPOCH,
                COMPOSITION_ID,
            )?
            .filter(|_| !generation_mismatch);
        let Some(selected_protocol) = selected else {
            return Ok((
                HostHandshake::Incompatible {
                    host_epoch: self.epoch.clone(),
                    protocol_min: 0,
                    protocol_max: 0,
                    compatibility_epoch: COMPATIBILITY_EPOCH,
                    composition_id: COMPOSITION_ID.into(),
                    composition_revision: "3".into(),
                    state: Lifecycle::Ready,
                    replacement: if ephemeral && settled {
                        Replacement::WaitForIdleExit
                    } else {
                        Replacement::BlockedByResidency
                    },
                    generation: self.options.generation.clone(),
                    activity: (local_owner
                        && (generation_mismatch || hello.activity_snapshot_version == Some(2)))
                    .then(|| self.activity_snapshot(hello.activity_snapshot_version)),
                },
                None,
                None,
            ));
        };
        self.accepted_connections.fetch_add(1, Ordering::SeqCst);
        Ok((
            HostHandshake::Accepted {
                root_id: self.root_id().into(),
                host_epoch: self.epoch.clone(),
                connection_id: connection_id.to_string(),
                selected_protocol,
                compatibility_epoch: COMPATIBILITY_EPOCH,
                composition_id: COMPOSITION_ID.into(),
                composition_revision: "3".into(),
                state: Lifecycle::Ready,
                cooperative_handoff: Some(true),
            },
            Some(ConnectionCount(&self.accepted_connections)),
            None,
        ))
    }

    fn activity_snapshot(&self, version: Option<u8>) -> Value {
        let activity = self.activity();
        let mut result = json!({
            "connections": self.accepted_connections.load(Ordering::SeqCst),
            "activeOperations": self.commands.len(),
            "processUptimeSeconds": self.started.elapsed().as_secs(),
            "residencies": activity.residencies(),
        });
        if version == Some(2) {
            result["drainResidencies"] = activity.resident_count().into();
        }
        result
    }
}
