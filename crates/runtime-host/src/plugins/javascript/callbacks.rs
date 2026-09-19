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

use maka_js_runtime::plugin::Module;
use maka_plugins::prompt::{Provider, Request, TextFuture};
use maka_runtime::tools::{
    PreparationFuture, PreparedEffect, ToolCallContext, ToolError, ToolPreparer,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) struct Callback {
    pub module: Module,
    pub id: u32,
    pub calls: Arc<super::invocation::Calls>,
}
impl Drop for Callback {
    fn drop(&mut self) {
        self.module.release_callback(self.id);
    }
}
pub(super) struct Tool {
    pub callback: Arc<Callback>,
    pub name: String,
}
impl ToolPreparer for Tool {
    fn names(&self) -> Vec<String> {
        vec![self.name.clone()]
    }
    fn prepare(
        &self,
        _: String,
        input: Value,
        context: ToolCallContext,
        _: CancellationToken,
    ) -> PreparationFuture {
        let callback = self.callback.clone();
        Box::pin(async move {
            Ok(PreparedEffect::new(move |cancellation| {
                Box::pin(async move {
                    let authority = callback.calls.enter(
                        super::invocation::Identity {
                            invocation: context.invocation.clone(),
                            operation_id: Some(context.operation_id.clone()),
                        },
                        cancellation.clone(),
                    )?;
                    let result = invoke(
                        &callback.module,
                        callback.id,
                        input,
                        json!({
                            "invocation": context.invocation, "operationId": context.operation_id,
                            "authority": authority.id,
                        }),
                        cancellation,
                    )
                    .await;
                    authority.finish().await?;
                    result.map(Into::into)
                })
            }))
        })
    }
}
pub(super) struct Prompt {
    pub callback: Arc<Callback>,
}
impl Provider for Prompt {
    fn evaluate(&self, request: Request) -> TextFuture {
        let callback = self.callback.clone();
        Box::pin(async move {
            let value = invoke(
                &callback.module,
                callback.id,
                serde_json::to_value(request.target)
                    .map_err(|e| maka_plugins::Error::Invalid(e.to_string()))?,
                Value::Null,
                request.cancellation,
            )
            .await
            .map_err(|error| maka_plugins::Error::Invalid(error.to_string()))?;
            match value {
                Value::Null => Ok(None),
                Value::String(value) => Ok(Some(value)),
                _ => Err(maka_plugins::Error::Invalid(
                    "prompt callback must return string or undefined".into(),
                )),
            }
        })
    }
}
pub(super) async fn invoke(
    module: &Module,
    callback: u32,
    input: Value,
    context: Value,
    cancellation: CancellationToken,
) -> Result<Value, ToolError> {
    let id = uuid::Uuid::new_v4().to_string();
    let request = module.call(
        vec!["invoke".into()],
        vec![json!(callback), input, context, json!(id)],
    );
    tokio::pin!(request);
    let result = tokio::select! {
        result = &mut request => result,
        _ = cancellation.cancelled() => {
            let drain = async {
                if let Err(error) = module.cancel_call(id).await {
                    module.terminate_vm(format!("cannot signal plugin cancellation: {error}"));
                }
                request.await
            };
            match tokio::time::timeout(Duration::from_secs(5), drain).await {
                Ok(result) => result,
                Err(_) => {
                    module.terminate_vm("plugin callback ignored cancellation");
                    return Err(ToolError::OutcomeUnknown("plugin callback did not settle after cancellation".into()));
                }
            }
        }
    };
    result.map_err(|error| {
        if module.vm_failed() {
            ToolError::OutcomeUnknown(error.to_string())
        } else {
            ToolError::Failed(error.to_string())
        }
    })
}
