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
use maka_plugins::executor::{Context, Error, Outcome, OutputSink, Provider, Request};
use maka_runtime::executor::Output;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Outputs(Mutex<BTreeMap<String, Arc<dyn OutputSink>>>);
impl Outputs {
    pub async fn emit(&self, handle: &str, output: Output) -> Result<(), Error> {
        let sink = self
            .0
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or(Error::Retired)?;
        sink.emit(output).await
    }
    fn register(self: &Arc<Self>, sink: Arc<dyn OutputSink>) -> Result<Handle, Error> {
        let mut sinks = self.0.lock().unwrap();
        if sinks.len() >= 128 {
            return Err(Error::Invalid("executor call capacity exceeded".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        sinks.insert(id.clone(), sink);
        Ok(Handle {
            outputs: self.clone(),
            id,
        })
    }
}
struct Handle {
    outputs: Arc<Outputs>,
    id: String,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.outputs.0.lock().unwrap().remove(&self.id);
    }
}
pub(super) struct Executor {
    pub callback: Arc<Callback>,
    pub outputs: Arc<Outputs>,
}
impl Provider for Executor {
    fn execute(
        &self,
        request: Request,
        context: Context,
    ) -> BoxFuture<'static, Result<Outcome, Error>> {
        let callback = self.callback.clone();
        let outputs = self.outputs.clone();
        Box::pin(async move {
            let handle = outputs.register(context.output)?;
            let authority = callback
                .calls
                .enter(
                    super::invocation::Identity {
                        invocation: request.invocation.clone(),
                        operation_id: None,
                    },
                    context.cancellation.clone(),
                )
                .map_err(|error| Error::Provider(error.to_string()))?;
            let input = json!({
                "invocation":request.invocation, "conversationKey":request.conversation_key,
                "content":request.content, "cwd":request.cwd, "instructions":request.instructions,
            });
            let result = invoke(
                &callback.module,
                callback.id,
                input,
                json!({"executor":handle.id, "authority":authority.id, "invocation":request.invocation}),
                context.cancellation,
            )
            .await;
            authority
                .finish()
                .await
                .map_err(|_| Error::CleanupUnconfirmed)?;
            let result = result.map_err(|error| match error {
                maka_runtime::tools::ToolError::OutcomeUnknown(_) => Error::CleanupUnconfirmed,
                other => Error::Provider(other.to_string()),
            })?;
            serde_json::from_value(result).map_err(|error| Error::Invalid(error.to_string()))
        })
    }
}
