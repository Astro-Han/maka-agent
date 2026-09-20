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

use crate::{
    Error,
    assignment::{Route, Target},
    invalid,
};
use maka_plugins::{
    authorization,
    execution::{CommandError, OfferInteraction, Prompt},
};
use maka_runtime::{
    capability::{FormField, FormFieldSpec, FormOption, FormResult, FormValue},
    event::Invocation,
    input::MessageInput,
    interaction::InteractionOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Selection {
    operation: String,
    source: Invocation,
    content: MessageInput,
    choices: Vec<Choice>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Choice {
    reference: String,
    session_id: String,
    label: String,
}

impl crate::plugin::Manager {
    pub(super) async fn selection(
        &self,
        operation: String,
        source: Invocation,
        revision: String,
        references: Vec<String>,
        text: String,
    ) -> Result<Selection, Error> {
        if !(2..=16).contains(&references.len())
            || references
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != references.len()
        {
            return Err(invalid("Select 2–16 distinct candidates"));
        }
        let candidates = self
            .assignments
            .repository
            .read::<crate::candidates::Candidates>("candidates")
            .await?
            .ok_or(Error::Conflict)?
            .1;
        if candidates.revision != revision {
            return Err(Error::Conflict);
        }
        let choices = references
            .into_iter()
            .map(|reference| {
                let candidate = candidates
                    .entries
                    .iter()
                    .find(|entry| entry.reference == reference)
                    .ok_or(Error::Conflict)?;
                let session = &candidate.summary.session;
                let label = format!(
                    "{reference}: {} — {}",
                    session.name, session.workspace.host_cwd
                );
                // Form bounds are bytes, not Unicode scalar count.
                let end = label.floor_char_boundary(190.min(label.len()));
                Ok(Choice {
                    reference,
                    session_id: session.session_id.clone(),
                    label: label[..end].into(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Selection {
            operation,
            content: self.delegation_content(&source, &text).await?,
            source,
            choices,
        })
    }

    pub(super) async fn execute_selection(
        &self,
        selection: Selection,
        cancellation: Option<CancellationToken>,
    ) -> Result<Value, Error> {
        let (_, commands) = self.coordinator.resolve().await?;
        let operation = format!(
            "selection:{}",
            crate::repository::digest(&selection.operation)?
        );
        let offer = commands
            .offer_interaction(OfferInteraction {
                operation_id: operation.clone(),
                invocation: selection.source.clone(),
                prompt: Prompt::Form {
                    message:
                        "Choose the work to continue. Selection does not grant execution access."
                            .into(),
                    fields: vec![FormField {
                        name: "target".into(),
                        label: "Work / Workspace".into(),
                        required: true,
                        description: None,
                        spec: FormFieldSpec::SingleSelect {
                            options: selection
                                .choices
                                .iter()
                                .map(|choice| FormOption {
                                    value: choice.reference.clone(),
                                    label: choice.label.clone(),
                                })
                                .collect(),
                            default: None,
                        },
                    }],
                },
            })
            .await?;
        let outcome = match (offer.outcome, cancellation) {
            (Some(outcome), _) => outcome,
            (None, Some(cancellation)) => tokio::select! {
                result = commands.wait_interaction(operation) => result?,
                _ = cancellation.cancelled() => return Err(Error::Execution(CommandError::Unavailable("Selection observation cancelled".into()))),
            },
            (None, None) => return Err(Error::Execution(CommandError::Busy)),
        };
        let reference = match outcome {
            InteractionOutcome::FormAnswer {
                result: FormResult::Accept { mut values },
                ..
            } => match values.remove("target") {
                Some(FormValue::String(reference)) => reference,
                _ => return Err(invalid("Selection answer has no target")),
            },
            InteractionOutcome::Closure { .. }
            | InteractionOutcome::FormAnswer {
                result: FormResult::Cancel | FormResult::Decline,
                ..
            } => return Ok(json!({"kind":"cancelled"})),
            _ => return Err(invalid("Unexpected selection answer")),
        };
        let target = selection
            .choices
            .into_iter()
            .find(|choice| choice.reference == reference)
            .ok_or(Error::Conflict)?;
        // The answer addresses the stored choice, not the latest candidate catalog.
        let receipt = self
            .assignments
            .route(Route {
                operation_id: selection.operation,
                source: selection.source,
                authorization: authorization::Target::Session {
                    session_id: target.session_id.clone(),
                },
                target: Target::Existing {
                    session_id: target.session_id,
                },
                content: selection.content,
            })
            .await?;
        serde_json::to_value(receipt).map_err(invalid)
    }
}
