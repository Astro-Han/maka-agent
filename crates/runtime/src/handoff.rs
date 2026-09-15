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

//! A physical Run can seal without ending its logical Turn. Only the sealed
//! facts authorize the named successor; an in-memory reservation does not.

use crate::{event::Invocation, interaction::entity_id};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU16;

mod execution;
pub use execution::{HandoffExecution, HandoffTools};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffIntent {
    pub handoff_id: String,
    pub host_epoch: String,
    pub root_run_id: String,
    pub successor_run_id: String,
    pub successor_invocation_id: String,
    pub claim_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffPause {
    pub intent: HandoffIntent,
    /// Captured by the Engine, never restored from mutable Session policy.
    pub remaining_steps: NonZeroU16,
    pub execution: Box<HandoffExecution>,
}

impl HandoffIntent {
    pub fn validate(&self, source: &Invocation) -> Result<(), &'static str> {
        for id in [
            &self.handoff_id,
            &self.host_epoch,
            &self.root_run_id,
            &self.successor_run_id,
            &self.successor_invocation_id,
            &self.claim_id,
        ] {
            entity_id(id)?;
        }
        if self.successor_run_id == self.root_run_id
            || [&source.run_id, &source.invocation_id].contains(&&self.successor_run_id)
            || [&source.run_id, &source.invocation_id].contains(&&self.successor_invocation_id)
            || self.successor_run_id == self.successor_invocation_id
        {
            return Err("handoff must name a fresh physical Run and invocation");
        }
        Ok(())
    }

    pub fn successor(&self, source: &Invocation) -> Invocation {
        Invocation {
            session_id: source.session_id.clone(),
            turn_id: source.turn_id.clone(),
            run_id: self.successor_run_id.clone(),
            invocation_id: self.successor_invocation_id.clone(),
        }
    }
}

impl HandoffPause {
    pub fn validate_claim(
        &self,
        claim: &crate::continuation::ContinuationClaim,
        target: &Invocation,
    ) -> Result<(), &'static str> {
        claim.validate_boundary(target)?;
        self.validate(&claim.source.invocation)?;
        if claim.id != self.intent.claim_id
            || claim.replay != self.execution.replay
            || self
                .execution
                .replay_base
                .is_some_and(|base| base != claim.base.high_water)
            || *target != self.intent.successor(&claim.source.invocation)
        {
            return Err("handoff must acquire its reserved successor in the same Turn");
        }
        Ok(())
    }

    pub fn validate(&self, source: &Invocation) -> Result<(), &'static str> {
        self.intent.validate(source)?;
        self.execution.validate()?;
        if self.remaining_steps.get() > 256 {
            return Err("handoff exceeds the Engine step budget");
        }
        Ok(())
    }
}
