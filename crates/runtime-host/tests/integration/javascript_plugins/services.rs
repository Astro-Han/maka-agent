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

use futures_util::future::BoxFuture;
use maka_plugins::{
    contributions::Staged,
    kernel::{Definition, Plugin, PluginContext},
    services::{BoundServices, method},
};
use maka_runtime_host::plugins::Setup;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

struct Native;
impl Plugin for Native {
    fn activate(
        &self,
        context: PluginContext,
        _: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        Box::pin(async move {
            context
                .services
                .provide_method(
                    "example.native",
                    Arc::new(Forward {
                        services: context.services.clone(),
                        host: context
                            .host
                            .as_ref()
                            .ok_or("missing public Host capabilities")?
                            .clone(),
                    }),
                )
                .map_err(|error| error.to_string())?;
            Ok(Staged::default())
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Echo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inspect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resources: Option<maka_plugins::process::Command>,
}
struct Forward {
    services: BoundServices,
    host: maka_plugins::host::Services,
}
impl method::Method<Echo, Value> for Forward {
    fn call(
        &self,
        mut input: Echo,
        context: method::Context,
    ) -> BoxFuture<'_, Result<Value, method::Error>> {
        Box::pin(async move {
            let service = self
                .services
                .method("example.echo")
                .map_err(|error| method::Error::Failed(error.to_string()))?
                .ok_or(method::Error::Retired)?;
            if let Some(original) = &context.invocation {
                let foreign = maka_plugins::call::Issuer::default()
                    .issue(original.identity.clone(), context.cancellation.clone())
                    .unwrap();
                assert!(matches!(
                    self.host.executions.acquire(foreign.clone()).await,
                    Err(maka_plugins::execution::CommandError::Revoked)
                ));
                self.host
                    .executions
                    .acquire(original.clone())
                    .await
                    .map_err(|error| method::Error::Failed(error.to_string()))?
                    .validate_authority()
                    .await
                    .map_err(|error| method::Error::Failed(error.to_string()))?;
                let read =
                    maka_plugins::filesystem::Operation::Read(maka_runtime::read::ReadInput {
                        path: "native-proof.txt".into(),
                        offset: None,
                        limit: None,
                    });
                assert!(
                    matches!(self.host.files.invoke(foreign.clone(), read.clone()).await,
                    Err(maka_runtime::tools::ToolError::Failed(reason)) if reason.contains("foreign or closed"))
                );
                let page = self
                    .host
                    .files
                    .invoke(original.clone(), read)
                    .await
                    .map_err(|error| method::Error::Failed(error.to_string()))?;
                assert_eq!(
                    page.into_json()["content"],
                    "native and JS share authority\n"
                );
                let result: Result<Value, _> = service
                    .call(
                        json!({"inspect":true}),
                        Some(foreign),
                        context.cancellation.clone(),
                    )
                    .await;
                assert!(
                    matches!(result, Err(method::Error::Failed(reason)) if reason.contains("foreign or closed"))
                );
            }
            if let Some(mut command) = input.resources.take() {
                use maka_plugins::{process, terminal};
                let call = context.invocation.clone().ok_or_else(|| {
                    method::Error::Invalid("resources require an admitted caller".into())
                })?;
                let foreign = maka_plugins::call::Issuer::default()
                    .issue(call.identity.clone(), context.cancellation.clone())
                    .unwrap();
                assert!(matches!(
                    self.host
                        .processes
                        .spawn(foreign.clone(), command.clone())
                        .await,
                    Err(process::Error::Denied)
                ));
                assert!(matches!(
                    self.host
                        .terminals
                        .spawn(
                            foreign,
                            terminal::Spawn {
                                command: command.clone(),
                                size: terminal::Size::new(80, 24).unwrap()
                            }
                        )
                        .await,
                    Err(process::Error::Denied)
                ));
                let process = self
                    .host
                    .processes
                    .spawn(call.clone(), command.clone())
                    .await
                    .unwrap();
                process.io.write(b"ping native\n".to_vec()).await.unwrap();
                let mut output = Vec::new();
                while !String::from_utf8_lossy(&output).contains("protocol:ping native") {
                    let chunk = process
                        .io
                        .next()
                        .await
                        .unwrap()
                        .expect("native pipe closed");
                    if matches!(chunk.stream, process::Stream::Stdout) {
                        output.extend(chunk.bytes);
                    }
                }
                // Service settlement, not the JS adapter, owns this live process.
                command
                    .env
                    .insert("MAKA_PLUGIN_PTY_TEST_CHILD".into(), "1".into());
                let terminal = self
                    .host
                    .terminals
                    .spawn(
                        call,
                        terminal::Spawn {
                            command,
                            size: terminal::Size::new(80, 24).unwrap(),
                        },
                    )
                    .await
                    .unwrap();
                let receipt = terminal
                    .io
                    .control(terminal::Control {
                        text: "quit\r".into(),
                        size: Some(terminal::Size::new(91, 29).unwrap()),
                    })
                    .await
                    .unwrap();
                assert_eq!(receipt.accepted_bytes, 5);
                assert!(receipt.resized);
                assert!(matches!(
                    terminal.io.wait().await.unwrap(),
                    terminal::Outcome::Completed
                ));
                terminal.io.close().await.unwrap();
            }
            service
                .call(input, context.invocation, context.cancellation)
                .await
        })
    }
}

pub(super) fn setup() -> Setup {
    Setup {
        builtins: BTreeMap::from([("example.native".into(), Arc::new(Definition {
            id: "example.native".into(),
            revision: "native".into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Native),
        }))]),
        layers: BTreeMap::from([("example.native".into(), serde_json::from_value(json!([
            {"type":"insert","rootId":"profile","entry":{"id":"example.native","packageId":"example.native"}}
        ])).unwrap())]),
        ..Default::default()
    }
}
