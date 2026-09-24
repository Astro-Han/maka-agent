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

use super::{Entity, Kind, Manage, SandboxMode, Target};
use crate::app::{Action, App, ConnectionState};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::{
    Operation,
    configuration::policy::{RuntimePolicyMutationResult, RuntimePolicySnapshot},
};
use serde_json::json;

pub(in crate::pages::manage) struct State {
    generation: u64,
    requested: bool,
    pub revision: Option<u64>,
}
#[derive(Clone)]
pub struct Request {
    target: Target,
    generation: u64,
}
impl super::State {
    pub fn defaults(generation: u64) -> Self {
        Self {
            defaults: Some(State {
                generation,
                requested: false,
                revision: None,
            }),
            initial_mode: SandboxMode::WorkspaceWrite,
            mode: SandboxMode::WorkspaceWrite,
            initial_approval: maka_sandbox::Approval::OnRequest,
            approval: maka_sandbox::Approval::OnRequest,
            approvals: false,
        }
    }
}
pub async fn read(client: &Client) -> Result<RuntimePolicySnapshot, String> {
    let value = client
        .request(Operation::RuntimePolicyQuery, json!({}))
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
pub(in crate::pages::manage) async fn write(
    client: &Client,
    ticket: &super::super::Ticket,
) -> Result<RuntimePolicyMutationResult, RequestFailure> {
    let (Some(revision), Some(mode)) = (ticket.policy_revision, ticket.sandbox_mode) else {
        return Err(RequestFailure::NotDispatched(ClientError::Protocol(
            "Missing default sandbox basis".into(),
        )));
    };
    let value = client
        .request(
            Operation::RuntimePolicyMutate,
            json!({"expectedRevision":revision,
        "operation":{"kind":"set_chat_defaults","value":{"sandboxMode":mode}}}),
        )
        .await?;
    serde_json::from_value(value)
        .map_err(|e| RequestFailure::Unknown(ClientError::Protocol(e.to_string())))
}
impl App {
    pub fn sandbox_defaults_action(&self) -> Option<Action> {
        let ConnectionState::Connected { root_id, epoch } = &self.connection else {
            return None;
        };
        Some(Action::Manage(Manage::Open(
            Target {
                root: root_id.clone(),
                epoch: epoch.clone(),
                name: String::new(),
                entity: Entity::SandboxDefaults,
            },
            Kind::Sandbox,
        )))
    }
    pub fn sandbox_defaults_request(&mut self) -> Option<Request> {
        let dialog = self.management.dialog.as_ref()?;
        if !self.management_identity(&dialog.target) || dialog.blocked {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let state = dialog.sandbox.as_mut()?.defaults.as_mut()?;
        if state.requested {
            return None;
        }
        state.requested = true;
        Some(Request {
            target: dialog.target.clone(),
            generation: state.generation,
        })
    }
    pub fn sandbox_defaults_completed(
        &mut self,
        request: Request,
        result: Result<RuntimePolicySnapshot, String>,
    ) {
        if !self.management_identity(&request.target) {
            return;
        }
        let Some(dialog) = self
            .management
            .dialog
            .as_mut()
            .filter(|d| d.target == request.target)
        else {
            return;
        };
        let Some(state) = dialog.sandbox.as_mut().filter(|s| {
            s.defaults
                .as_ref()
                .is_some_and(|d| d.generation == request.generation)
        }) else {
            return;
        };
        match result {
            Ok(snapshot) => {
                state.mode = snapshot.policy.chat_defaults.sandbox_mode;
                state.initial_mode = state.mode;
                state.defaults.as_mut().unwrap().revision = Some(snapshot.revision);
                dialog.focus = state.initial_focus();
            }
            Err(_) => {
                dialog.blocked = true;
                dialog.error = Some("sandbox-default-failed");
            }
        }
        dialog.visible = false;
        self.hits.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, i18n::I18n};

    #[test]
    fn default_read_cannot_seed_a_reopened_dialog_or_enable_unseen_saves() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        let open = app.sandbox_defaults_action().unwrap();
        app.apply(open.clone());
        let first = app.sandbox_defaults_request().unwrap();
        assert!(app.management_request().is_none());
        app.apply(Action::Manage(Manage::Close));
        app.apply(open);
        let second = app.sandbox_defaults_request().unwrap();
        let snapshot = RuntimePolicySnapshot {
            revision: 17,
            policy: Default::default(),
        };
        app.sandbox_defaults_completed(first, Ok(snapshot.clone()));
        assert!(
            !app.management
                .dialog
                .as_ref()
                .unwrap()
                .sandbox
                .as_ref()
                .unwrap()
                .loaded()
        );
        app.sandbox_defaults_completed(second, Ok(snapshot));
        let state = app
            .management
            .dialog
            .as_ref()
            .unwrap()
            .sandbox
            .as_ref()
            .unwrap();
        assert_eq!(state.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(app.management.dialog.as_ref().unwrap().focus, 1);
        assert_eq!(state.approval, maka_sandbox::Approval::OnRequest);
        assert!(!state.changed());
        assert!(app.management_request().is_none());
    }
}
