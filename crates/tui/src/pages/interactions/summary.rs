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

use super::{Review, State};
use crate::i18n::I18n;
use maka_protocol::{capability::form::FormResult, interaction::*};
use maka_sandbox::{Network, filesystem, grant};

pub(super) fn text(review: &Review, i18n: &I18n) -> String {
    let snapshot = &review.ticket.snapshot;
    if review.details {
        let mut text = format!(
            "{}\n\n{}\n{}",
            i18n.format(
                "interaction-identity",
                &[
                    ("session", snapshot.session_id()),
                    ("turn", snapshot.turn_id()),
                    ("run", snapshot.run_id()),
                    ("id", snapshot.interaction_id()),
                ]
            ),
            i18n.text("interaction-details"),
            serde_json::to_string_pretty(snapshot.request()).expect("wire request"),
        );
        if let Some(outcome) = &review.outcome {
            text.push_str(&format!(
                "\n\n{}\n{}",
                i18n.text("interaction-receipt"),
                serde_json::to_string_pretty(&outcome.outcome()).expect("wire outcome")
            ));
        }
        if let Some(error) = &review.error {
            text.push_str(&format!("\n\n{error}"));
        }
        return text;
    }
    let mut lines = vec![];
    match snapshot.request() {
        InteractionRequest::Permissions { request, .. } => {
            lines.push(request.reason.clone());
            if let Some(command) = &request.command {
                lines.push(format!(
                    "{} · {}",
                    i18n.text("review-command"),
                    command.command
                ));
                lines.push(format!(
                    "{} · {}",
                    i18n.text("review-directory"),
                    command.cwd
                ));
            }
            if review.state != State::Resolved {
                lines.push(i18n.text("review-permissions"));
                permissions(&mut lines, &request.permissions, i18n);
            }
        }
        InteractionRequest::ClientCapability { target, .. } => {
            lines.push(format!("{} · {}", target.tool_name, target.server_id));
            lines.push(format!(
                "{} · {}",
                i18n.text("review-provider"),
                target.provider_id
            ));
            lines.push(i18n.text("review-session-grant"));
            let scope = match &target.scope {
                GrantScope::BrowserOrigin { origin } => origin.clone(),
                GrantScope::Capability {} => i18n.text("review-whole-capability"),
                GrantScope::McpTool {
                    server_id,
                    tool_name,
                } => format!("{server_id} / {tool_name}"),
            };
            lines.push(format!("{} · {scope}", i18n.text("review-scope")));
        }
        InteractionRequest::Question { questions, .. } => {
            lines.extend(questions.iter().map(|question| question.question.clone()));
        }
        InteractionRequest::Form {
            message, requester, ..
        } => {
            lines.push(message.clone());
            lines.push(requester.name.clone());
        }
    }
    if let Some(outcome) = review
        .outcome
        .as_ref()
        .and_then(|snapshot| snapshot.outcome())
    {
        lines.push(String::new());
        let key = match outcome {
            InteractionOutcome::PermissionsDecision {
                decision:
                    grant::Decision::Allow {
                        permissions: granted,
                        scope,
                    },
                ..
            } => {
                permissions(&mut lines, granted, i18n);
                match scope {
                    grant::Scope::Once => "review-allowed-once",
                    grant::Scope::Turn => "review-allowed-turn",
                    grant::Scope::Session => "review-allowed-session",
                }
            }
            InteractionOutcome::PermissionsDecision {
                decision: grant::Decision::Deny,
                ..
            }
            | InteractionOutcome::ClientCapabilityDecision {
                decision: Decision::Deny,
                ..
            } => "review-denied",
            InteractionOutcome::ClientCapabilityDecision {
                decision: Decision::Allow,
                ..
            } => "review-allowed-session",
            InteractionOutcome::QuestionAnswer { .. } => "review-answers-submitted",
            InteractionOutcome::FormAnswer { result, .. } => match result {
                FormResult::Accept { .. } => "review-form-submitted",
                FormResult::Decline => "review-form-declined",
                FormResult::Cancel => "review-form-cancelled",
            },
            InteractionOutcome::Closure { reason, .. } => match reason {
                ClosureReason::TurnStopped => "review-turn-stopped",
                ClosureReason::TurnTerminal => "review-turn-ended",
                ClosureReason::ProducerCancelled => "review-withdrawn",
                ClosureReason::TimedOut => "review-expired",
                ClosureReason::HostRestarted => "review-host-restarted",
                ClosureReason::ProviderDisconnected => "review-provider-disconnected",
            },
        };
        lines.push(i18n.text(key));
    }
    if let Some(error) = &review.error {
        lines.push(String::new());
        lines.push(error.lines().next().unwrap_or(error).into());
    }
    lines.join("\n")
}

fn permissions(lines: &mut Vec<String>, permissions: &grant::Permissions, i18n: &I18n) {
    for rule in &permissions.filesystem {
        let access = i18n.text(match rule.access {
            filesystem::Access::Read => "review-read",
            filesystem::Access::Write => "review-write",
            filesystem::Access::Deny => "review-no-access",
        });
        let scope = i18n.text(match rule.scope {
            filesystem::Scope::Exact => "review-exact",
            filesystem::Scope::Subtree => "review-subtree",
        });
        lines.push(format!("{access} · {scope} · {}", rule.path.display()));
    }
    match &permissions.network {
        Network::Denied => lines.push(i18n.text("review-network-denied")),
        Network::Allowed => lines.push(i18n.text("review-network-allowed")),
        Network::Restricted { destinations } => {
            lines.push(i18n.text("review-network-restricted"));
            lines.extend(
                destinations.iter().map(|destination| {
                    format!("  {} : {}", destination.host(), destination.port())
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Locale, LocalePreference,
        pages::interactions::{Command, tests::fixture},
    };

    #[test]
    fn compact_review_keeps_permission_boundaries_and_details_never_answer_or_rebind() {
        let mut app = fixture();
        app.open_interaction();
        let original = app.interactions.review.as_ref().unwrap().ticket.clone();
        let plain = text(app.interactions.review.as_ref().unwrap(), &app.i18n);
        for expected in [
            "Write 中文🦀 file",
            "printf test",
            "/tmp/approval/output",
            "Read and write",
            "Exact path",
            "Network · no access",
        ] {
            assert!(plain.contains(expected), "{plain}");
        }
        assert!(!plain.contains("baseRevision") && !plain.contains("toolUseId"));
        assert!(app.interaction_request(Command::Details).is_none());
        assert_eq!(app.interactions.review.as_ref().unwrap().ticket, original);
        let detail = text(app.interactions.review.as_ref().unwrap(), &app.i18n);
        assert!(detail.contains("baseRevision") && detail.contains("Request: approval"));
        assert!(app.interaction_request(Command::Details).is_none());
        let (ticket, answer) = app.interaction_request(Command::Once).unwrap();
        let receipt = InteractionSnapshot::from_record(&InteractionRecord {
            session_id: ticket.snapshot.session_id().into(),
            turn_id: ticket.snapshot.turn_id().into(),
            run_id: ticket.snapshot.run_id().into(),
            request_id: ticket.snapshot.interaction_id().into(),
            created_at: 0,
            request: ticket.snapshot.request().clone(),
            outcome: Some(answer.unwrap().into_outcome(1)),
        })
        .unwrap();
        app.interaction_completed(ticket, Ok(receipt));
        for locale in Locale::ALL {
            app.i18n.preference = LocalePreference::Explicit(locale);
            let review = app.interactions.review.as_ref().unwrap();
            assert_eq!(review.state, State::Resolved);
            let plain = text(review, &app.i18n);
            assert!(plain.contains(&app.i18n.text("review-allowed-once")));
            assert!(!plain.contains("committedAt") && !plain.contains("toolUseId"));
            let requested = grant::Permissions {
                filesystem: vec![
                    filesystem::Rule::subtree("/tmp/tree", filesystem::Access::Read),
                    filesystem::Rule::exact("/tmp/blocked", filesystem::Access::Deny),
                ],
                network: Network::destination(
                    maka_sandbox::Destination::new("example.com", 443).unwrap(),
                ),
            };
            let mut lines = vec![];
            permissions(&mut lines, &requested, &app.i18n);
            let shown = lines.join("\n");
            assert!(
                shown.contains(&app.i18n.text("review-subtree")) && shown.contains("/tmp/tree")
            );
            assert!(
                shown.contains(&app.i18n.text("review-no-access"))
                    && shown.contains("/tmp/blocked")
            );
            assert!(
                shown.contains(&app.i18n.text("review-network-restricted"))
                    && shown.contains("example.com : 443")
            );
            assert!(app.i18n.diagnostics().is_empty());
        }
        assert!(app.interaction_request(Command::Details).is_none());
        assert!(text(app.interactions.review.as_ref().unwrap(), &app.i18n).contains("committedAt"));
        assert_eq!(app.interactions.review.as_ref().unwrap().ticket, original);
        assert!(
            app.interaction_request(Command::Once).is_none(),
            "reading a receipt cannot grant again"
        );
    }
}
