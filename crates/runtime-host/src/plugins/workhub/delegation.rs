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

use super::{
    control::{Result, failure, fingerprint},
    selection::SelectedTarget,
    target::Target,
};
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{ActInput, ActResult, Proposal},
};
use maka_runtime::workhub::{ActionId, Delegation, DelegationKind};

#[derive(Clone)]
pub(crate) struct Identity {
    pub action_id: ActionId,
    pub turn_id: String,
    pub fingerprint: String,
}
pub(crate) struct Request {
    pub identity: Identity,
    pub target: Target,
    pub selected: Option<SelectedTarget>,
    pub text: Option<String>,
}

impl super::Control {
    pub(crate) async fn delegate(
        &self,
        input: ActInput,
        selected: Option<SelectedTarget>,
    ) -> Result<ActResult> {
        if !matches!(input.proposal, Proposal::Route(_)) {
            return Err(failure(
                Code::OperationConflict,
                "Not an ordinary WorkHub delegation",
            ));
        }
        let identity = Identity {
            action_id: input.action_id.clone(),
            turn_id: input.turn_id.clone(),
            fingerprint: fingerprint(&input)?,
        };
        if let Some(delegation) = self
            .commands
            .delegation(self.caller.clone(), identity.clone())
            .await?
        {
            return Ok(receipt(&delegation));
        }
        let target = self.prepare_target(&input, selected.as_ref()).await?;
        let delegation = self
            .commands
            .delegate(
                self.caller.clone(),
                Request {
                    identity,
                    target,
                    selected,
                    text: input.delegation_text,
                },
            )
            .await?;
        Ok(receipt(&delegation))
    }
}

fn receipt(delegation: &Delegation) -> ActResult {
    let target_session_id = delegation.target.session_id.clone();
    let target_turn_id = delegation.target.turn_id.clone();
    match delegation.kind {
        DelegationKind::Existing => ActResult::DelegateExisting {
            target_session_id,
            target_turn_id,
            steered: delegation.delivery.is_steering(),
        },
        DelegationKind::Created => ActResult::CreateNew {
            target_session_id,
            target_turn_id,
        },
    }
}
