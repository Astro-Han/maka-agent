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
    control::{Identity, Result, failure},
    target::Target,
};
use maka_event_log::workhub::correction::{CorrectionRecord, CorrectionResolution};
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{
        ActInput, ActResult, DelegationDisposition, LinkedProposal, Proposal, RoutingProposal,
    },
};
use maka_runtime::{
    artifact::content_digest,
    workhub::{ActionId, CorrectionTarget, CreateSpec, DelegationKind},
};

pub(crate) mod recovery;

pub(crate) struct Request {
    pub identity: Identity,
    pub replaces_action_id: ActionId,
    pub target: Target,
    pub text: Option<String>,
}

impl super::Control {
    pub(crate) async fn correct(&self, input: ActInput) -> Result<ActResult> {
        let Proposal::Linked(LinkedProposal::Correct {
            replaces_action_id, ..
        }) = &input.proposal
        else {
            return Err(failure(Code::OperationConflict, "Not a WorkHub correction"));
        };
        if let Some(record) = self
            .commands
            .correction(
                self.caller.clone(),
                input.action_id.clone(),
                input.turn_id.clone(),
            )
            .await?
        {
            let identity = identity(&input, &record.intent.request.target)?;
            validate_receipt(&identity, &record)?;
            self.verify_correction_replay(&input, &record.intent.request.target)
                .await?;
            if record.resolution.is_some() {
                return receipt(record);
            }
            return receipt(self.commands.settle_correction(identity).await?);
        }
        let target = self.prepare_target(&input, None).await?;
        let identity = identity(&input, &target.correction())?;
        let record = self
            .commands
            .correct(
                self.caller.clone(),
                Request {
                    identity,
                    replaces_action_id: replaces_action_id.clone(),
                    target,
                    text: input.delegation_text,
                },
            )
            .await?;
        receipt(record)
    }

    async fn verify_correction_replay(
        &self,
        input: &ActInput,
        target: &CorrectionTarget,
    ) -> Result<()> {
        let Proposal::Linked(LinkedProposal::Correct {
            target: RoutingProposal::DelegateExisting { candidate_ref },
            ..
        }) = &input.proposal
        else {
            return Ok(());
        };
        let candidates = match self.candidates().await {
            Ok(candidates) => candidates,
            // A draining Host can still report an already durable receipt.
            Err(error) if error.code == Code::HostDraining => return Ok(()),
            Err(error) => return Err(error),
        };
        if input.candidate_set_id.as_ref() == Some(&candidates.result.candidate_set_id)
            && candidates
                .result
                .candidates
                .iter()
                .find(|candidate| &candidate.candidate_ref == candidate_ref)
                .is_none_or(|candidate| candidate.session_id != target.session_id())
        {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub correction cannot redirect its admitted target",
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_receipt(identity: &Identity, record: &CorrectionRecord) -> Result<()> {
    let request = &record.intent.request;
    if request.action_id != identity.action_id
        || request.source.turn_id != identity.turn_id
        || request.request_fingerprint != identity.fingerprint
    {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub correction belongs to another request",
        ));
    }
    Ok(())
}

fn identity(input: &ActInput, target: &CorrectionTarget) -> Result<Identity> {
    #[derive(serde::Serialize)]
    enum Choice<'a> {
        Existing(&'a str),
        Created(CreateSpec),
    }
    let Proposal::Linked(LinkedProposal::Correct {
        replaces_action_id,
        target: route,
    }) = &input.proposal
    else {
        return Err(failure(Code::OperationConflict, "Not a WorkHub correction"));
    };
    let choice = match route {
        RoutingProposal::DelegateExisting { .. } if target.create().is_none() => {
            Choice::Existing(target.session_id())
        }
        RoutingProposal::CreateNew { title } if target.create().is_some() => {
            Choice::Created(CreateSpec {
                title: title.clone(),
                workspace: input
                    .create
                    .as_ref()
                    .ok_or_else(|| {
                        failure(
                            Code::OperationConflict,
                            "WorkHub creation context is missing",
                        )
                    })?
                    .workspace
                    .clone(),
                defaults: input.new_work_defaults.clone(),
            })
        }
        _ => {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub correction target kind changed",
            ));
        }
    };
    let bytes = serde_json::to_vec(&(
        "workhub.correct.v1",
        &input.action_id,
        &input.turn_id,
        replaces_action_id,
        choice,
        &input.delegation_text,
    ))
    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
    Ok(Identity {
        action_id: input.action_id.clone(),
        turn_id: input.turn_id.clone(),
        fingerprint: content_digest(&bytes),
    })
}

fn receipt(record: CorrectionRecord) -> Result<ActResult> {
    match record.resolution {
        Some(CorrectionResolution::Assigned(assigned)) => Ok(ActResult::Replace {
            replacement_disposition: match assigned.delegation.kind {
                DelegationKind::Existing => DelegationDisposition::DelegateExisting,
                DelegationKind::Created => DelegationDisposition::CreateNew,
            },
            target_session_id: assigned.delegation.target.session_id.clone(),
            target_turn_id: assigned.delegation.target.turn_id.clone(),
            steered: assigned.delegation.delivery.is_steering(),
        }),
        Some(CorrectionResolution::Aborted(_)) => Err(failure(
            Code::OperationConflict,
            "WorkHub correction retired the old association, but the replacement target is unavailable",
        )),
        None => Err(failure(
            Code::OperationUnavailable,
            "WorkHub correction is awaiting retirement",
        )),
    }
}
