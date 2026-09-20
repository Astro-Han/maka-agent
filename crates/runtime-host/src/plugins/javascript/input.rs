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
use super::callbacks::{Callback, invoke};
use futures_util::future::BoxFuture;
use maka_plugins::{Error, input};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

pub(super) struct Input {
    pub callback: Arc<Callback>,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Outcome {
    Unchanged,
    Ready {
        content: maka_protocol::turn::MessageContent,
        receipt: Value,
        #[serde(default)]
        required_tools: BTreeSet<String>,
        basis: Option<super::revision::Reference>,
    },
    Blocked {
        message: String,
        receipt: Value,
    },
}
impl input::Provider for Input {
    fn prepare(
        &self,
        request: input::Request,
        workspace: maka_plugins::filesystem::ReadDirectory,
    ) -> BoxFuture<'static, Result<input::Outcome, Error>> {
        let callback = self.callback.clone();
        Box::pin(async move {
            let view = callback.calls.borrow_read(workspace)?;
            let content = maka_protocol::turn::MessageContent::from(request.content.clone());
            let value = invoke(
                &callback.module,
                callback.id,
                json!({ "sessionId": request.session_id, "cwd": request.cwd,
                    "content": content, "preparation": request.content.preparation,
                    "selections": request.selections, "tools": request.tools }),
                json!({"readView": view.id}),
                request.cancellation,
            )
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
            let outcome: Outcome =
                serde_json::from_value(value).map_err(|e| Error::Invalid(e.to_string()))?;
            Ok(match outcome {
                Outcome::Unchanged => input::Outcome::Unchanged,
                Outcome::Ready {
                    content,
                    receipt,
                    required_tools,
                    basis,
                } => input::Outcome::Ready {
                    content: content.into(),
                    receipt,
                    required_tools,
                    basis: basis
                        .map(|basis| callback.calls.revisions.basis(basis))
                        .transpose()?,
                },
                Outcome::Blocked { message, receipt } => {
                    input::Outcome::Blocked { message, receipt }
                }
            })
        })
    }
}
