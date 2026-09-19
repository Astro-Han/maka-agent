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
    pub name: String,
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
        text: String,
        receipt: Value,
        #[serde(default)]
        required_tools: BTreeSet<String>,
    },
    Blocked {
        message: String,
        receipt: Value,
    },
}
impl input::Provider for Input {
    fn prepare(
        &self,
        mut request: input::Request,
    ) -> BoxFuture<'static, Result<input::Outcome, Error>> {
        let callback = self.callback.clone();
        let selections = request.selections.remove(&self.name).unwrap_or_default();
        Box::pin(async move {
            let content = maka_protocol::turn::MessageContent::from(request.content.clone());
            let value = invoke(
                &callback.module,
                callback.id,
                json!({ "sessionId": request.session_id, "cwd": request.cwd,
                    "content": content, "selections": selections, "tools": request.tools }),
                Value::Null,
                request.cancellation,
            )
            .await
            .map_err(|e| Error::Invalid(e.to_string()))?;
            let outcome: Outcome =
                serde_json::from_value(value).map_err(|e| Error::Invalid(e.to_string()))?;
            Ok(match outcome {
                Outcome::Unchanged => input::Outcome::Unchanged,
                Outcome::Ready {
                    text,
                    receipt,
                    required_tools,
                } => {
                    // Only prepared text changes. Attachments, original input and
                    // prior providers' evidence retain their canonical ownership.
                    request.content.text = text;
                    input::Outcome::Ready {
                        content: request.content,
                        receipt,
                        required_tools,
                        basis: None,
                    }
                }
                Outcome::Blocked { message, receipt } => {
                    input::Outcome::Blocked { message, receipt }
                }
            })
        })
    }
}
