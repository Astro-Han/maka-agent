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
    ActiveRun, Executions, Result, failure, internal, prepare::MessageOrigin, requires_drain,
};
use maka_agent::RunningInvocation;
use maka_protocol::{OperationErrorCode as Code, turn::TurnStartInput};
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
    message::{self, MessageDisposition as Disposition, RootSourceMessage},
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl Executions {
    /// Dispatch accepted pending work unless an existing worker owns its cleanup.
    pub(crate) async fn dispatch_pending<'a>(
        self: &'a Arc<Self>,
        session: &str,
        admission: &mut Option<tokio::sync::MutexGuard<'a, ()>>,
    ) -> Result<()> {
        if self.has_active_session(session) {
            return Ok(());
        }
        match self.next_message(session, admission, None).await {
            Ok(Some(running)) => self.track(running),
            Ok(None) => {}
            Err(error) => {
                self.begin_drain();
                return Err(failure(
                    Code::HostDraining,
                    &format!("Pending work startup failed: {}", error.message),
                ));
            }
        }
        Ok(())
    }

    /// One worker owns the whole chain; root ownership survives cleanup and
    /// the gate-protected handoff. Never await effects while holding the gate.
    pub(super) async fn drive(self: Arc<Self>, mut running: RunningInvocation) {
        loop {
            let invocation = running.invocation().clone();
            let outcome = running.wait().await;
            let mut admission = Some(self.lock_admission().await);
            if let Err(error) = outcome {
                if requires_drain(&error) {
                    self.begin_drain();
                }
                eprintln!("execution {} ended: {error}", invocation.run_id);
            }
            let next = if self.shutdown.is_cancelled() {
                Ok(None)
            } else {
                self.next_message(&invocation.session_id, &mut admission, Some(&invocation))
                    .await
            };
            let mut active = self.active.lock().unwrap();
            if let Some(previous) = active.remove(&invocation.run_id) {
                previous.completed.cancel();
            }
            match next {
                Ok(Some(next)) => {
                    active.insert(
                        next.invocation().run_id.clone(),
                        ActiveRun {
                            invocation: next.invocation().clone(),
                            tool_names: next.tool_names().clone(),
                            cancellation: next.cancellation(),
                            completed: CancellationToken::new(),
                        },
                    );
                    running = next;
                }
                Ok(None) => break,
                Err(error) => {
                    self.begin_drain();
                    eprintln!("message handoff failed: {}", error.message);
                    break;
                }
            }
        }
    }

    pub(super) async fn next_message<'a>(
        &'a self,
        session: &str,
        admission: &mut Option<tokio::sync::MutexGuard<'a, ()>>,
        owner: Option<&Invocation>,
    ) -> Result<Option<RunningInvocation>> {
        let mut environment = None;
        loop {
            if self.shutdown.is_cancelled() {
                return Ok(None);
            }
            // The previous worker retains cleanup ownership while preparation
            // yields admission. An unowned pending root may have been picked up
            // by another worker; only that worker may now perform its handoff.
            if self
                .active
                .lock()
                .unwrap()
                .values()
                .any(|run| run.invocation.session_id == session && Some(&run.invocation) != owner)
            {
                return Ok(None);
            }
            let queue = self.log.pending_messages(session).await.map_err(internal)?;
            if queue.is_empty() {
                return Ok(None);
            }
            if self
                .log
                .has_pending_handoff(session)
                .await
                .map_err(internal)?
            {
                return Ok(None);
            }
            let Some((basis, candidate)) = environment.take() else {
                let record = self
                    .log
                    .get_session::<crate::session::SessionConfiguration>(session)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
                let basis = record.configuration_digest.clone();
                drop(admission.take());
                let candidate = self
                    .prepare_environment_for(
                        record,
                        None,
                        maka_client_capability::BindingMode::Degrade,
                    )
                    .await;
                *admission = Some(self.lock_admission().await);
                environment = Some((basis, candidate));
                continue;
            };
            let candidate = match candidate {
                Ok(candidate) => match candidate.commit(self, session).await {
                    Ok(Some(candidate)) => Ok(candidate),
                    Ok(None) => continue,
                    Err(error) => Err(error),
                },
                Err(error) => {
                    let current = self
                        .log
                        .get_session::<crate::session::SessionConfiguration>(session)
                        .await
                        .map_err(internal)?;
                    if current.is_some_and(|record| record.configuration_digest != basis) {
                        continue;
                    }
                    Err(error)
                }
            };
            let sources = successor_sources(&queue)?;
            let content = message::aggregate(sources.iter().map(|source| &source.message.content));
            let invocation = if sources[0].disposition == Disposition::TurnStarted {
                queue[0].invocation.clone()
            } else {
                Invocation {
                    session_id: session.into(),
                    turn_id: Uuid::new_v4().to_string(),
                    run_id: Uuid::new_v4().to_string(),
                    invocation_id: Uuid::new_v4().to_string(),
                }
            };
            let intent = sources[0].submitted_intent.as_ref();
            let prepared = match candidate {
                Err(error) => Err(error),
                Ok(environment) => {
                    self.prepare_message(
                        TurnStartInput {
                            session_id: session.into(),
                            turn_id: invocation.turn_id.clone(),
                            content: content.into(),
                            // Accepted source content already contains the frozen
                            // instructions; recovery must not load it a second time.
                            skill_ids: intent
                                .filter(|_| sources[0].skill_invocation.loaded.is_empty())
                                .map(|intent| intent.skill_ids.clone()),
                            turn_orchestration: intent
                                .and_then(|intent| intent.turn_orchestration.clone()),
                            max_steps: None,
                        },
                        MessageOrigin::Successor,
                        None,
                        sources.clone(),
                        environment,
                    )
                    .await
                }
            };
            let prepared = prepared.and_then(|(input, _)| {
                super::skills::validate_pending_tools(&input, &queue, &sources)?;
                Ok(input)
            });
            let cancellation = self.shutdown.child_token();
            let result = match prepared {
                Ok(mut input) => {
                    input.invocation = invocation.clone();
                    self.engine
                        .start(input, cancellation.clone())
                        .await
                        .map_err(|error| {
                            if requires_drain(&error) {
                                self.begin_drain();
                            }
                            super::execution_error(error)
                        })
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(running) => return Ok(Some(running)),
                Err(error) if self.shutdown.is_cancelled() => return Err(error),
                Err(error) => {
                    // Already accepted work keeps a canonical owner even if its
                    // current model route or workspace can no longer be prepared.
                    self.fail_pending_root(invocation, sources, error.message)
                        .await?;
                }
            }
        }
    }

    async fn fail_pending_root(
        &self,
        invocation: Invocation,
        sources: Vec<RootSourceMessage>,
        reason: String,
    ) -> Result<()> {
        let facts = [
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    content: message::aggregate(
                        sources.iter().map(|source| &source.message.content),
                    ),
                    source_messages: sources,
                    skill_invocation: Default::default(),
                    request_fingerprint: None,
                },
            },
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Failed {
                    class: "message_preparation_failed".into(),
                    message: Some(reason),
                },
            },
        ];
        let events = facts
            .into_iter()
            .map(|fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(internal)?;
        self.log.append_batch(&events).await.map_err(|error| {
            self.begin_drain();
            failure(Code::OutcomeUnknown, &error.to_string())
        })?;
        Ok(())
    }
}

/// Late steering precedes explicit followups, but independently accepted rows
/// can span multiple canonical roots. Never discard or over-aggregate that tail.
fn successor_sources(
    queue: &[maka_event_log::message_admissions::PendingMessageAdmission],
) -> Result<Vec<RootSourceMessage>> {
    let mut sources = Vec::new();
    for entry in queue
        .iter()
        .filter(|entry| entry.source.disposition == Disposition::Steering)
    {
        sources.push(entry.source.clone());
        let content = message::aggregate(sources.iter().map(|source| &source.message.content));
        if let Err(reason) = message::validate_sources(&content, &sources) {
            if sources.len() == 1 {
                return Err(failure(Code::InternalFailure, reason));
            }
            sources.pop();
            break;
        }
    }
    if sources.is_empty() {
        sources.extend(queue.first().map(|entry| entry.source.clone()));
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_event_log::message_admissions::PendingMessageAdmission;
    use maka_runtime::{input::DeliveredMessage, message::Placement};

    #[test]
    fn successor_capacity_preserves_steering_order_and_explicit_followup_boundaries() {
        let entry = |id: &str, disposition, bytes| PendingMessageAdmission {
            invocation: Invocation {
                session_id: "session".into(),
                turn_id: "turn".into(),
                run_id: "run".into(),
                invocation_id: "invocation".into(),
            },
            steering_invocation: None,
            required_tools: Default::default(),
            admitted_at: 1,
            source: RootSourceMessage {
                message: DeliveredMessage {
                    message_id: id.into(),
                    content: "a".repeat(bytes).into(),
                    submitted_content_digest: format!("sha256:{}", "a".repeat(64)),
                },
                submitted_placement: if disposition == Disposition::Followup {
                    Placement::NextTurn
                } else {
                    Placement::CurrentTurn
                },
                disposition,
                skill_invocation: Default::default(),
                submitted_intent: None,
            },
        };
        let queue = vec![
            entry("followup", Disposition::Followup, 1),
            entry("first", Disposition::Steering, 40 * 1024),
            entry("second", Disposition::Steering, 40 * 1024),
            entry("third", Disposition::Steering, 1),
        ];
        let first = successor_sources(&queue).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|s| s.message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
        let mut remaining = queue;
        remaining.remove(1);
        let second = successor_sources(&remaining).unwrap();
        assert_eq!(
            second
                .iter()
                .map(|s| s.message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        let content = message::aggregate(second.iter().map(|s| &s.message.content));
        message::validate_sources(&content, &second).unwrap();
        assert_eq!(
            successor_sources(&remaining[..1]).unwrap()[0]
                .message
                .message_id,
            "followup"
        );
    }
}
