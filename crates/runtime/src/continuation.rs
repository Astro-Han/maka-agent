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

//! Durable continuation identity. The opening itself acquires the claim;
//! a query or an in-memory plan has no execution authority.

use crate::{archive::valid_projection_digest, event::Invocation, interaction::entity_id};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBoundary {
    pub invocation: Invocation,
    /// Run-local ordinal, never a root ledger sequence.
    pub high_water: u64,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBase {
    /// Root ledger position in the source's Session, before the first fresh Run.
    pub high_water: u64,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationClaim {
    pub id: String,
    pub source: RunBoundary,
    pub base: SessionBase,
    /// Engine-owned provider replay evidence, separate from the raw source digest.
    pub replay: ReplayEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEvidence {
    pub version: u32,
    pub digest: String,
    pub route_identity: String,
}

pub const REPLAY_VERSION: u32 = 1;
pub const MAX_ANCESTRY: usize = 64;
pub const MAX_SOURCE_EVENTS: usize = 10_000;
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;

impl ReplayEvidence {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.version != REPLAY_VERSION
            || !valid_projection_digest(&self.digest)
            || !valid_projection_digest(&self.route_identity)
        {
            return Err("invalid provider replay evidence");
        }
        Ok(())
    }
}

impl ContinuationClaim {
    pub fn validate(&self, target: &Invocation) -> Result<(), &'static str> {
        self.validate_boundary(target)?;
        if self.source.invocation.turn_id == target.turn_id {
            return Err("manual continuation requires a fresh Turn");
        }
        Ok(())
    }

    pub(crate) fn validate_boundary(&self, target: &Invocation) -> Result<(), &'static str> {
        entity_id(&self.id)?;
        for invocation in [&self.source.invocation, target] {
            for id in [
                &invocation.session_id,
                &invocation.turn_id,
                &invocation.run_id,
                &invocation.invocation_id,
            ] {
                entity_id(id)?;
            }
        }
        let source = &self.source.invocation;
        if source.session_id != target.session_id
            || source.run_id == target.run_id
            || source.invocation_id == target.invocation_id
        {
            return Err("continuation requires fresh Run and Invocation in the same Session");
        }
        if self.source.high_water == 0
            || self.source.high_water > crate::configuration::validation::MAX_SAFE_INTEGER
            || self.base.high_water >= i64::MAX as u64
            || !valid_projection_digest(&self.source.digest)
            || !valid_projection_digest(&self.base.digest)
        {
            return Err("invalid continuation boundary or replay evidence");
        }
        self.replay.validate()
    }
}
