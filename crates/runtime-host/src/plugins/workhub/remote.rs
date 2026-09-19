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

//! Desktop calls carry their transport origin, never an Agent invocation.
use super::{Control, answer::Request};
use futures_util::future::BoxFuture;
use maka_plugins::{
    contributions::Staged,
    remote::{Caller, Endpoint, Error, Handler, Method, key},
};
use maka_protocol::{Outcome, workhub};
use maka_runtime::workhub::COORDINATION_SESSION_ID;
use serde_json::Value;
use std::sync::Arc;

pub(super) fn publish(
    staged: &mut Staged,
    control: &Control,
    content_digest: &str,
) -> Result<(), String> {
    for (name, action) in [
        ("resolve", Action::Resolve),
        ("query", Action::Query),
        ("answer", Action::Answer),
        ("enqueue", Action::Enqueue),
        ("answer-receipt", Action::AnswerReceipt),
        ("configure-model", Action::ConfigureModel),
        ("feedback", Action::Feedback),
    ] {
        staged
            .insert(
                key(super::ID, name).map_err(message)?,
                Endpoint::new(
                    content_digest.into(),
                    Handler::Method(Arc::new(Call {
                        control: control.clone(),
                        action,
                    })),
                ),
            )
            .map_err(message)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Action {
    Resolve,
    Query,
    Answer,
    Enqueue,
    AnswerReceipt,
    ConfigureModel,
    Feedback,
}
struct Call {
    control: Control,
    action: Action,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Enqueue {
    origin_host_epoch: String,
    expected_turn_id: String,
    message_id: String,
    content: maka_protocol::turn::MessageContent,
    placement: maka_protocol::message::Placement,
}

impl Method for Call {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let control = self.control.clone();
        let action = self.action;
        Box::pin(async move {
            if caller
                .session_id
                .as_deref()
                .is_some_and(|id| id != COORDINATION_SESSION_ID)
            {
                return Err(Error::Invalid(
                    "WorkHub requires its coordination Session".into(),
                ));
            }
            if caller.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let result = match action {
                Action::Enqueue => {
                    let request: Enqueue = serde_json::from_value(input).map_err(invalid)?;
                    let mut input = maka_protocol::message::SubmitInput {
                        origin_host_epoch: request.origin_host_epoch,
                        session_id: COORDINATION_SESSION_ID.into(),
                        message_id: request.message_id,
                        content: request.content,
                        placement: request.placement,
                        skill_ids: None,
                        turn_orchestration: None,
                    };
                    input.validate().map_err(invalid)?;
                    control
                        .commands
                        .enqueue(
                            control.caller.clone(),
                            input,
                            request.expected_turn_id,
                            caller.connection_id,
                            caller.cancellation,
                        )
                        .await
                        .and_then(encode)
                }
                Action::Resolve => {
                    empty(&input)?;
                    control.resolve(caller.cancellation).await.and_then(encode)
                }
                Action::Query => {
                    empty(&input)?;
                    control.query().await.and_then(encode)
                }
                Action::Feedback => {
                    let references = serde_json::from_value(input).map_err(invalid)?;
                    control
                        .feedback(references, caller.cancellation)
                        .await
                        .and_then(encode)
                }
                Action::AnswerReceipt => {
                    let input = workhub::decode_answer_input(&input).map_err(invalid)?;
                    let request = Request::new(input).map_err(|error| invalid(error.message))?;
                    control
                        .commands
                        .answer_receipt(&request)
                        .await
                        .and_then(encode)
                }
                Action::Answer => {
                    let input = workhub::decode_answer_input(&input).map_err(invalid)?;
                    let request = Request::new(input).map_err(|error| invalid(error.message))?;
                    control
                        .answer(request, caller.connection_id, caller.cancellation)
                        .await
                        .and_then(encode)
                }
                Action::ConfigureModel => {
                    let input = workhub::decode_model_input(&input).map_err(invalid)?;
                    control
                        .configure_model(input, caller.cancellation)
                        .await
                        .and_then(encode)
                }
            };
            // Domain outcomes retain conflict/busy/unknown-commit semantics.
            // Remote transport failure is not proof that a mutation rolled back.
            serde_json::to_value(match result {
                Ok(value) => Outcome::success(value),
                Err(error) => Outcome::failure(error),
            })
            .map_err(|error| Error::Provider(error.to_string()))
        })
    }
}
fn empty(input: &Value) -> Result<(), Error> {
    if input.is_null() {
        Ok(())
    } else {
        Err(invalid("This WorkHub method accepts null input"))
    }
}
fn encode(value: impl serde::Serialize) -> Result<Value, maka_protocol::OperationError> {
    serde_json::to_value(value).map_err(|error| {
        super::control::failure(
            maka_protocol::OperationErrorCode::InternalFailure,
            error.to_string(),
        )
    })
}
fn invalid(error: impl ToString) -> Error {
    Error::Invalid(error.to_string())
}
fn message(error: impl ToString) -> String {
    error.to_string()
}
