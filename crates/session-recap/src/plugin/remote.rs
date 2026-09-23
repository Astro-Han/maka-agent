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

use crate::recap::{Error as RecapError, Recaps};
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
    Generate { operation_id: uuid::Uuid },
}
pub(super) fn publish(
    backend: Arc<Recaps>,
    bundle: Option<&Bundle>,
    staged: &mut Staged,
) -> Result<(), String> {
    let service = Arc::new(Service(backend));
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
struct Service(Arc<Recaps>);
impl Method for Service {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let recaps = self.0.clone();
        Box::pin(async move {
            let request: Request = serde_json::from_value(input)
                .map_err(|_| Error::Invalid("Invalid recap request".into()))?;
            let session = caller
                .session_id
                .as_deref()
                .ok_or_else(|| Error::Invalid("Bind recap to a Session".into()))?;
            let generating = matches!(request, Request::Generate { .. });
            let operation_id = match &request {
                Request::Generate { operation_id } => *operation_id,
                Request::Read => uuid::Uuid::new_v4(),
            };
            let capabilities = if generating {
                [Capability::ReadHistory, Capability::Models].into()
            } else {
                [Capability::ReadHistory].into()
            };
            let owned = caller
                .views
                .authorize(Authorization {
                    operation_id,
                    title: if generating {
                        "Generate Session recap"
                    } else {
                        "Read Session recap"
                    }
                    .into(),
                    target: Target::Session {
                        session_id: session.into(),
                    },
                    capabilities,
                })
                .await?;
            let result = match request {
                Request::Read => recaps.read(&owned.scope(), session).await,
                Request::Generate { operation_id } => recaps
                    .generate(&owned.scope(), session, operation_id)
                    .await
                    .map(Some),
            };
            owned
                .finish()
                .await
                .map_err(|_| Error::OutcomeUnknown("Recap cleanup is unconfirmed".into()))?;
            result
                .map(|recap| json!({"recap":recap}))
                .map_err(|e| match e {
                    RecapError::Unknown => Error::OutcomeUnknown(e.to_string()),
                    _ => Error::Provider(e.to_string()),
                })
        })
    }
}
fn message(e: impl std::fmt::Display) -> String {
    e.to_string()
}
