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

//! AI SDK wire representation stays behind this adapter.

use crate::{ModelError, ToolDefinition};
use maka_js_runtime::trusted::TrustedRuntime;
use maka_plugins::model::{Context, Lifetime, ProviderAdapter, Request, Session};
use maka_runtime::model::request::Request as ModelRequest;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

mod chat;

fn serialize_tools<S: serde::Serializer>(
    tools: &[ToolDefinition],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "kebab-case")]
    enum SdkTool<'a> {
        Function(&'a ToolDefinition),
        Provider {
            id: &'a str,
            name: &'a str,
            args: &'a Value,
        },
    }
    serializer.collect_seq(tools.iter().map(|tool| match &tool.provider {
        Some(provider) => SdkTool::Provider {
            id: &provider.id,
            name: &tool.name,
            args: &provider.args,
        },
        None => SdkTool::Function(tool),
    }))
}

pub(super) async fn stream(
    runtime: TrustedRuntime,
    mut request: ModelRequest,
    context: Context,
) -> Result<(), ModelError> {
    let Context {
        events: sender,
        cancellation,
        idle_timeout: _,
        transport: network,
    } = context;
    #[derive(Serialize)]
    struct Tools<'a>(#[serde(serialize_with = "serialize_tools")] &'a [ToolDefinition]);
    if matches!(
        request.provider.kind,
        crate::ProviderKind::OpenaiChat | crate::ProviderKind::OpenaiCompatible { .. }
    ) {
        request.prompt = chat::project(request.prompt);
    }
    let tools = serde_json::to_value(Tools(&request.tools))
        .map_err(|error| ModelError::Adapter(error.to_string()))?;
    let mut request =
        serde_json::to_value(request).map_err(|error| ModelError::Adapter(error.to_string()))?;
    request["tools"] = tools;
    let (events, mut receiver) = mpsc::channel(1);
    let execution = runtime.model(request, events, cancellation.clone(), network);
    tokio::pin!(execution);
    let mut normalizer = crate::events::Normalizer::default();
    let forward = async {
        while let Some(event) = receiver.recv().await {
            sender.progress();
            let event = normalizer.push(event.map_err(ModelError::from)?.into_value())?;
            if let Some(event) = event {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(ModelError::Cancelled),
                    sent = sender.emit(event) => sent?,
                }
            }
        }
        Ok(normalizer)
    };
    tokio::pin!(forward);
    // Both futures are owned here: returning an adapter error also cancels and
    // settles the SDK worker before releasing the Host's execution permit.
    tokio::select! {
        result = &mut execution => {
            result.map_err(ModelError::from)?;
            forward.await?.end()
        }
        result = &mut forward => {
            match result {
                Err(error) => {
                    cancellation.cancel();
                    let _ = execution.await;
                    Err(error)
                }
                Ok(normalizer) => {
                    execution.await.map_err(ModelError::from)?;
                    normalizer.end()
                },
            }
        }
    }
}

pub struct Adapter(pub TrustedRuntime);
impl ProviderAdapter for Adapter {
    fn open(
        &self,
        _: Lifetime,
        _: tokio_util::sync::CancellationToken,
    ) -> futures_util::future::BoxFuture<'_, Result<Arc<dyn Session>, ModelError>> {
        let session = Sdk(self.0.clone());
        Box::pin(async { Ok(Arc::new(session) as Arc<dyn Session>) })
    }
}
struct Sdk(TrustedRuntime);
impl Session for Sdk {
    fn stream(
        &self,
        request: Request,
        context: Context,
    ) -> futures_util::future::BoxFuture<'static, Result<(), ModelError>> {
        Box::pin(stream(self.0.clone(), request, context))
    }
}
