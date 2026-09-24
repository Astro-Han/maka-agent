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
    goal::{Arm, Control, Error as GoalError},
    owner::Owner,
};
use futures_util::future::BoxFuture;
use maka_plugins::{
    authorization::{Capability, Request as Authorization, Target},
    client::Bundle,
    contributions::Staged,
    remote::{Caller, Endpoint, Error, Handler, Method, key},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Request {
    Read,
    Arm {
        arm: Arm,
    },
    Control {
        id: uuid::Uuid,
        revision: u64,
        action: Control,
        grant: Option<maka_plugins::authorization::Id>,
    },
}
pub(super) fn publish(
    owner: Arc<Owner>,
    bundle: Option<&Bundle>,
    staged: &mut Staged,
) -> Result<(), String> {
    let service = Arc::new(Service(owner));
    staged
        .insert(
            key(super::ID, "manage").map_err(message)?,
            Endpoint::standalone(Handler::Method(service.clone())),
        )
        .map_err(message)?;
    if let Some(bundle) = bundle {
        staged
            .insert(
                key(super::ID, "request").map_err(message)?,
                Endpoint::new(bundle.content_digest.clone(), Handler::Method(service)),
            )
            .map_err(message)?;
    }
    Ok(())
}
pub(super) struct Service(pub(super) Arc<Owner>);
impl Method for Service {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let owner = self.0.clone();
        Box::pin(async move {
            let request: Request =
                serde_json::from_value(input).map_err(|e| Error::Invalid(e.to_string()))?;
            let session = caller
                .session_id
                .as_deref()
                .ok_or_else(|| Error::Invalid("Bind Goal to a Session".into()))?;
            let owned = caller
                .views
                .authorize(Authorization {
                    operation_id: uuid::Uuid::new_v4(),
                    title: "Manage Session Goal".into(),
                    target: Target::Session {
                        session_id: session.into(),
                    },
                    capabilities: [Capability::Executions].into(),
                })
                .await?;
            let result = async {
                // Probe canonical Session existence and current boundary even for cached reads.
                let commands = owner
                    .executions
                    .acquire(owned.scope())
                    .await
                    .map_err(GoalError::from)?;
                commands.session(session.into()).await?;
                match request {
                    Request::Read => {}
                    Request::Arm { arm } => {
                        owner.arm(session, arm).await?;
                    }
                    Request::Control {
                        id,
                        revision,
                        action,
                        grant,
                    } => {
                        owner.control(session, id, revision, action, grant).await?;
                    }
                }
                let saved = owner.repo.read(session).await?;
                Ok::<_, GoalError>(saved.map(|s| json!({"revision":s.revision,"goal":s.goal})))
            }
            .await;
            owned
                .finish()
                .await
                .map_err(|_| Error::CleanupUnconfirmed)?;
            result
                .map(|goal| json!({"current":goal}))
                .map_err(|e| match e {
                    GoalError::Storage(maka_plugins::storage::StoreError::OutcomeUnknown(_))
                    | GoalError::Authority(
                        maka_plugins::execution::CommandError::OutcomeUnknown(_),
                    ) => Error::OutcomeUnknown(e.to_string()),
                    _ => Error::Provider(e.to_string()),
                })
        })
    }
}
fn message(e: impl std::fmt::Display) -> String {
    e.to_string()
}
