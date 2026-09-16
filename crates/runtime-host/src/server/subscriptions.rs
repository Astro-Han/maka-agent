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

pub(super) mod delivery;
mod pty;
mod streams;
mod tools;
mod transcript;

use super::{Host, HostError};
use crate::session::SessionConfiguration;
use delivery::Delivery;
use maka_protocol::subscription::*;
use maka_protocol::transcript::decode_session_transcript_page_input;
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(super) type Registry = Arc<Mutex<HashSet<String>>>;

/// The connection writer owns delivery and activation. Drop only removes
/// observation registrations; it neither cancels nor releases execution tasks.
pub(super) struct Subscriptions {
    registry: Registry,
    owned: BTreeMap<String, Delivery>,
    pty: pty::PtyObservers,
}

impl Subscriptions {
    pub fn poll_pty(
        &mut self,
        host: &Host,
        outbound: &super::outbound::Outbound,
    ) -> Result<Option<Value>, HostError> {
        self.pty.poll(host, outbound, |id| {
            self.owned.get(id).is_some_and(|delivery| delivery.ready)
        })
    }
    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }

    pub fn resource_changed(
        &mut self,
        host: &Host,
        change: &maka_event_log::shell_runs::ShellChange,
    ) -> Result<(), HostError> {
        self.pty.refresh(host, change);
        self.owned
            .values_mut()
            .try_for_each(|delivery| delivery.resource_changed(change))
    }

    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            owned: BTreeMap::new(),
            pty: Default::default(),
        }
    }

    pub async fn dispatch(
        &mut self,
        host: &Host,
        operation: Operation,
        input: Value,
        outbound: &super::outbound::Outbound,
    ) -> Result<Outcome, HostError> {
        match operation {
            Operation::SubscriptionPtyInterestSet => {
                let input = decode_pty_interest_input(&input)?;
                let Some(delivery) = self.owned.get(&input.subscription_id) else {
                    return Ok(failure(
                        Code::NotFound,
                        "Session subscription was not found",
                    ));
                };
                self.pty.set(
                    host,
                    &input.subscription_id,
                    delivery.session_id(),
                    &input.refs,
                );
                outbound.retain_pty(&input.subscription_id, &input.refs);
                Ok(Outcome::success(serde_json::to_value(
                    SubscriptionCloseResult {
                        subscription_id: input.subscription_id,
                    },
                )?))
            }
            Operation::SubscriptionOpen => {
                let input = decode_subscription_open_input(&input)?;
                if self.owned.len() >= 16 {
                    return Ok(failure(
                        Code::OperationConflict,
                        "Connection subscription limit reached",
                    ));
                }
                let log = host.log.clone();
                let session_id = input.session_id.clone();
                let subscription_id = Uuid::new_v4().to_string();
                let id = subscription_id.clone();
                let policy = input.transcript.clone();
                let prepared = async {
                    let observation = log
                        .observe_session::<SessionConfiguration>(&session_id)
                        .await
                        .map_err(transcript::store_error)?
                        .ok_or(OperationError {
                            code: Code::NotFound,
                            message: "Session was not found".into(),
                        })?;
                    let transcript = transcript::prepare(&log, &observation, id, &policy).await?;
                    Ok::<_, OperationError>((observation, transcript))
                }
                .await;
                let (observation, transcript) = match prepared {
                    Ok(prepared) => prepared,
                    Err(mut error) => {
                        // open does not declare invalid_request: a valid input
                        // reaching that branch means an internal producer error.
                        if error.code == Code::InvalidRequest {
                            error.code = Code::InternalFailure;
                        }
                        return Ok(Outcome::failure(error));
                    }
                };
                let (delivery, output) =
                    Delivery::open(&host.epoch, &subscription_id, observation, transcript)?;
                output.validate_for(&input, &host.epoch)?;
                self.registry
                    .lock()
                    .map_err(|_| "subscription registry poisoned")?
                    .insert(subscription_id.clone());
                self.owned.insert(subscription_id, delivery);
                Ok(Outcome::success(serde_json::to_value(output)?))
            }
            Operation::SubscriptionClose => {
                let input = decode_subscription_close_input(&input)?;
                let mut registry = self
                    .registry
                    .lock()
                    .map_err(|_| "subscription registry poisoned")?;
                if self.owned.remove(&input.subscription_id).is_some() {
                    registry.remove(&input.subscription_id);
                    self.pty.remove(&input.subscription_id);
                    outbound.retain_pty(&input.subscription_id, &[]);
                } else if registry.contains(&input.subscription_id) {
                    return Ok(failure(
                        Code::NotFound,
                        "Session subscription was not found",
                    ));
                }
                Ok(Outcome::success(serde_json::to_value(input)?))
            }
            Operation::SessionTranscriptPage => {
                let input = decode_session_transcript_page_input(&input)?;
                let Some(delivery) = self.owned.get(&input.subscription_id) else {
                    return Ok(failure(
                        Code::NotFound,
                        "Session subscription was not found",
                    ));
                };
                let Some(access) = &delivery.transcript else {
                    return Ok(failure(
                        Code::OperationUnavailable,
                        "Subscription has no transcript access",
                    ));
                };
                if access.is_unavailable() {
                    return Ok(failure(
                        Code::OperationUnavailable,
                        "Transcript exceeds available presentation capacity",
                    ));
                }
                let result = access.state.page(&host.log, &input).await;
                match result {
                    Ok(page) => Ok(Outcome::success(serde_json::to_value(page)?)),
                    Err(error) => Ok(Outcome::failure(transcript::operation_error(error))),
                }
            }
            Operation::SubscriptionReady => {
                let input = decode_subscription_close_input(&input)?;
                let Some(delivery) = self.owned.get_mut(&input.subscription_id) else {
                    return Ok(failure(
                        Code::NotFound,
                        "Session subscription was not found",
                    ));
                };
                delivery.ready = true;
                Ok(Outcome::success(serde_json::to_value(input)?))
            }
            _ => unreachable!("subscription operation dispatch"),
        }
    }

    /// One bounded page per subscription leaves request/stop admission runnable
    /// between catch-up batches. A slow writer fails the connection, never skips
    /// a previously assigned sequence.
    pub async fn poll(&mut self, host: &Host) -> Result<(Vec<Value>, bool), HostError> {
        let mut frames = Vec::new();
        let mut more = false;
        if self.owned.is_empty() {
            return Ok((frames, more));
        }
        let sessions = self
            .owned
            .values()
            .map(|delivery| delivery.session_id().to_owned())
            .collect::<Vec<_>>();
        let versions = host.log.observation_versions(&sessions).await?;
        for delivery in self.owned.values_mut().filter(|delivery| delivery.ready) {
            let version = *versions
                .get(delivery.session_id())
                .ok_or("observed Session disappeared")?;
            if delivery.version == Some(version) {
                continue;
            }
            let (mut next, pending) = delivery.poll(host).await?;
            // A paged fence or unfinished transcript always continues, even if
            // no new fact arrived since the previous page.
            delivery.version = (!pending).then_some(version);
            frames.append(&mut next);
            more |= pending;
        }
        Ok((frames, more))
    }
}
impl Drop for Subscriptions {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            for id in self.owned.keys() {
                registry.remove(id);
            }
        }
    }
}

fn failure(code: Code, message: &str) -> Outcome {
    Outcome::failure(OperationError {
        code,
        message: message.into(),
    })
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> maka_protocol::Result<Value> {
    match operation {
        Operation::SubscriptionOpen => {
            decode_subscription_open_input(value)?;
        }
        Operation::SubscriptionClose => {
            decode_subscription_close_input(value)?;
        }
        Operation::SubscriptionPtyInterestSet => {
            decode_pty_interest_input(value)?;
        }
        Operation::SessionTranscriptPage => {
            decode_session_transcript_page_input(value)?;
        }
        Operation::SubscriptionReady => {
            decode_subscription_close_input(value)?;
        }
        _ => unreachable!("subscription operation decoder"),
    }
    Ok(value.clone())
}

pub(super) fn errors(operation: Operation) -> Option<&'static [Code]> {
    match operation {
        Operation::SessionTranscriptPage => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::InvalidRequest,
            Code::NotFound,
            Code::OperationConflict,
            Code::PersistenceFailed,
            Code::InternalFailure,
        ]),
        Operation::SubscriptionOpen => Some(&[
            Code::TranscriptPreparing,
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::OperationConflict,
            Code::PersistenceFailed,
            Code::InternalFailure,
        ]),
        Operation::SubscriptionClose
        | Operation::SubscriptionReady
        | Operation::SubscriptionPtyInterestSet => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::InternalFailure,
        ]),
        _ => None,
    }
}
