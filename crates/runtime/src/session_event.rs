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

//! Session-owned control facts share log order without extending a sealed Run.
use crate::workhub::ActionId;
use crate::{
    event::Invocation,
    workhub::{
        COORDINATION_SESSION_ID, CorrectionAbort, CorrectionIntent, Delegation, StopIntent,
        StopResolution,
    },
};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionEvent {
    pub id: String,
    pub recorded_at: SystemTime,
    pub session_id: String,
    /// Correlation with the user decision, not execution ownership.
    pub turn_id: String,
    pub fact: SessionFact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionFact {
    WorkhubCorrectionRequested {
        intent: Box<CorrectionIntent>,
    },
    WorkhubDelegated {
        coordinator: Invocation,
        delegation: Box<Delegation>,
        replaces_action_id: ActionId,
    },
    WorkhubSuperseded {
        action_id: ActionId,
        replaces_action_id: ActionId,
        replacement_delegation_id: String,
    },
    WorkhubCorrectionAborted {
        action_id: ActionId,
        reason: CorrectionAbort,
    },
    WorkhubStopRequested {
        intent: Box<StopIntent>,
        target_session_name: String,
        user_text: String,
    },
    WorkhubStopResolved {
        action_id: ActionId,
        resolution: StopResolution,
    },
}

impl SessionEvent {
    pub fn workhub(turn_id: String, fact: SessionFact) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            recorded_at: SystemTime::now(),
            session_id: COORDINATION_SESSION_ID.into(),
            turn_id,
            fact,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        crate::interaction::entity_id(&self.id)?;
        crate::interaction::entity_id(&self.turn_id)?;
        if self.session_id != COORDINATION_SESSION_ID {
            return Err("invalid WorkHub control owner");
        }
        match &self.fact {
            SessionFact::WorkhubCorrectionRequested { intent } => {
                intent.validate()?;
                if intent.request.source.turn_id != self.turn_id {
                    return Err("invalid WorkHub correction correlation");
                }
            }
            SessionFact::WorkhubDelegated {
                coordinator,
                delegation,
                ..
            } => {
                delegation.validate(coordinator)?;
                if coordinator.turn_id != self.turn_id {
                    return Err("invalid WorkHub assignment correlation");
                }
            }
            SessionFact::WorkhubSuperseded {
                replacement_delegation_id,
                ..
            } => {
                crate::interaction::entity_id(replacement_delegation_id)?;
            }
            SessionFact::WorkhubCorrectionAborted { .. } => {}
            SessionFact::WorkhubStopRequested {
                intent,
                target_session_name,
                user_text,
            } => {
                intent.validate()?;
                if intent.request.source.turn_id != self.turn_id
                    || target_session_name.trim().is_empty()
                    || target_session_name.len() > 4096
                    || user_text.trim().is_empty()
                    || user_text.len() > 64 * 1024
                {
                    return Err("invalid WorkHub stop request evidence");
                }
            }
            SessionFact::WorkhubStopResolved { resolution, .. } => {
                resolution.validate()?;
            }
        }
        Ok(())
    }
}

impl SessionFact {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WorkhubCorrectionRequested { .. } => "workhub_correction_requested",
            Self::WorkhubDelegated { .. } => "workhub_delegated",
            Self::WorkhubSuperseded { .. } => "workhub_superseded",
            Self::WorkhubCorrectionAborted { .. } => "workhub_correction_aborted",
            Self::WorkhubStopRequested { .. } => "workhub_stop_requested",
            Self::WorkhubStopResolved { .. } => "workhub_stop_resolved",
        }
    }
}
