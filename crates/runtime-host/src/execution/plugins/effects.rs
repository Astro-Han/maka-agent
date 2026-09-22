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

mod client;
mod http;
mod notification;
use super::Executions;
use futures_util::future::BoxFuture;
use maka_config::plugin_authorization::Boundary;
use maka_event_log::effects::{Operation, Outcome, Request};
use maka_fs_tools::{MutationExecutor, ReadExecutor, ReadLimits, ReadOutput};
use maka_plugins::{
    authorization::Capability,
    call::Scope,
    fiber::Context,
    filesystem::{Operation as File, Output},
    llm::{Generate, ModelGeneration},
};
use maka_runtime::tools::{ToolError, ToolExecutor};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

struct Prepared<T> {
    operation: Operation,
    capability: Capability,
    effect: BoxFuture<'static, Result<T, ToolError>>,
}
type Evidence = (Outcome, Option<Vec<u8>>);
trait Recorded {
    fn evidence(&self) -> Result<Evidence, ToolError>;
}
impl Recorded for Output {
    fn evidence(&self) -> Result<Evidence, ToolError> {
        Ok(match self {
            Self::Value(value) => (
                Outcome::Completed {
                    value: value.clone(),
                },
                None,
            ),
            Self::Entries(value) => (
                Outcome::Completed {
                    value: serde_json::to_value(value).map_err(failed)?,
                },
                None,
            ),
            Self::Image { bytes, mime_type } => (
                Outcome::Image {
                    mime_type: mime_type.clone(),
                    digest: maka_runtime::artifact::content_digest(bytes),
                },
                Some(bytes.clone()),
            ),
        })
    }
}
impl Recorded for ModelGeneration {
    fn evidence(&self) -> Result<Evidence, ToolError> {
        let value = serde_json::to_value(self)
            .map_err(|error| ToolError::Persistence(error.to_string()))?;
        Ok((Outcome::Completed { value }, None))
    }
}
impl Recorded for serde_json::Value {
    fn evidence(&self) -> Result<Evidence, ToolError> {
        Ok((
            Outcome::Completed {
                value: self.clone(),
            },
            None,
        ))
    }
}

impl Executions {
    pub(crate) async fn plugin_resource_file(
        &self,
        owner: Context,
        call: Scope,
        input: File,
        cancellation: CancellationToken,
    ) -> Result<Output, ToolError> {
        let capability = if input.is_read() {
            Capability::ReadFiles
        } else {
            Capability::WriteFiles
        };
        let boundary = self
            .plugin_resource_boundary(&call, capability)
            .await
            .map_err(ToolError::from)?;
        let effect = self
            .prepare_resource_file(boundary, input.clone(), cancellation.clone())
            .await?;
        self.journal(
            owner,
            call,
            Prepared {
                operation: Operation::File(input),
                capability,
                effect,
            },
            cancellation,
        )
        .await
    }
    pub(crate) async fn plugin_resource_model(
        self: &Arc<Self>,
        owner: Context,
        call: Scope,
        input: Generate,
        cancellation: CancellationToken,
    ) -> Result<ModelGeneration, ToolError> {
        let capability = Capability::Models;
        let boundary = self
            .plugin_resource_boundary(&call, capability)
            .await
            .map_err(ToolError::from)?;
        let prepared = self
            .prepare_resource_model(boundary, &call.identity, input, cancellation.clone())
            .await?;
        self.journal(owner, call, prepared, cancellation).await
    }
    /// The common SDK worker owns this one-shot operation even when its caller
    /// disappears. No Agent invocation is fabricated for user/background work.
    async fn journal<T: Recorded>(
        &self,
        owner: Context,
        call: Scope,
        prepared: Prepared<T>,
        cancellation: CancellationToken,
    ) -> Result<T, ToolError> {
        let _lease = owner.admit().map_err(failed)?;
        // Preparation may inspect files or provider configuration. Only the
        // final revalidation and durable admission hold the Host-wide gate.
        let gate = self.interactions.own_admission().await;
        let boundary = self
            .plugin_resource_boundary(&call, prepared.capability)
            .await
            .map_err(ToolError::from)?;
        let _admitted = owner.admit().map_err(failed)?;
        let request = Request {
            source: call.identity.clone(),
            owner: owner.identity().map_err(failed)?,
            boundary,
            operation: prepared.operation,
        };
        let id = self.log.begin_host_effect(request).await.map_err(|error| {
            self.begin_drain();
            ToolError::Persistence(error.to_string())
        })?;
        drop(gate);
        let result = if cancellation.is_cancelled() {
            Err(failed("cancelled before effect"))
        } else {
            prepared.effect.await
        };
        let (outcome, payload) = match &result {
            Ok(value) => value.evidence().inspect_err(|_| {
                self.begin_drain();
            })?,
            Err(error @ (ToolError::Failed(_) | ToolError::Io { .. })) => (
                Outcome::Failed {
                    message: error.to_string(),
                },
                None,
            ),
            Err(error) => (
                Outcome::Unknown {
                    message: error.to_string(),
                },
                None,
            ),
        };
        self.log
            .settle_host_effect(id, outcome, payload)
            .await
            .map_err(|error| {
                self.begin_drain();
                ToolError::Persistence(error.to_string())
            })?;
        if matches!(
            result,
            Err(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))
        ) {
            self.begin_drain();
        }
        result
    }

    async fn prepare_resource_file(
        &self,
        boundary: Boundary,
        input: File,
        cancellation: CancellationToken,
    ) -> Result<BoxFuture<'static, Result<Output, ToolError>>, ToolError> {
        let state_root = self.paths.state_root.clone();
        let (root, directory, policy) = tokio::task::spawn_blocking(move || {
            let origin = match &boundary {
                Boundary::Session { boundary, .. } => boundary.workspace_origin,
                Boundary::Workspace { origin, .. } => *origin,
                _ => maka_runtime::execution::WorkspaceOrigin::Selected,
            };
            let mode = match &boundary {
                Boundary::Session { boundary, .. } => boundary.sandbox_mode,
                Boundary::Workspace { sandbox_mode, .. } => *sandbox_mode,
                Boundary::Directory { .. } => {
                    maka_runtime::execution::SandboxMode::DangerFullAccess
                }
                Boundary::Profile => maka_runtime::execution::SandboxMode::ReadOnly,
            };
            let (path, directory) = match boundary {
                Boundary::Directory { path, identity } => {
                    let directory =
                        maka_fs_tools::directory::open(std::path::Path::new(&path), &identity)?;
                    (path, directory)
                }
                Boundary::Session {
                    boundary,
                    workspace_identity,
                } => {
                    let directory = maka_fs_tools::workspace::open_directory(
                        std::path::Path::new(&boundary.cwd),
                        &workspace_identity,
                    )?;
                    (boundary.cwd, directory)
                }
                Boundary::Workspace {
                    workspace,
                    workspace_identity,
                    ..
                } => {
                    let directory = maka_fs_tools::workspace::open_directory(
                        std::path::Path::new(&workspace.host_cwd),
                        &workspace_identity,
                    )?;
                    (workspace.host_cwd, directory)
                }
                Boundary::Profile => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "file access requires a directory grant",
                    ));
                }
            };
            let root = PathBuf::from(path);
            let (sandbox, _) =
                crate::execution::permissions::resolve(mode, &root, &state_root, origin)
                    .map_err(std::io::Error::other)?;
            let policy = match sandbox {
                maka_sandbox::Sandbox::Managed { filesystem, .. } => Some(Arc::new(
                    filesystem.compile().map_err(std::io::Error::other)?,
                )),
                _ => None,
            };
            Ok::<_, std::io::Error>((root, directory, policy))
        })
        .await
        .map_err(failed)?
        .map_err(failed)?;
        if let File::Entries(operation) = input {
            return Ok(Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    if cancellation.is_cancelled() {
                        return Err(failed("file operation cancelled before effect"));
                    }
                    let ceiling = policy.as_deref().map(|filesystem| {
                        maka_plugins::filesystem::entries::Policy {
                            root: &root,
                            filesystem,
                        }
                    });
                    maka_plugins::filesystem::entries::execute(
                        &directory,
                        operation,
                        &cancellation,
                        ceiling,
                    )
                    .map(Output::Entries)
                    .map_err(super::filesystem::entry_error)
                })
                .await
                .map_err(failed)?
            }));
        }
        let write = matches!(input, File::Write(_) | File::Edit(_) | File::Patch(_));
        let read = ReadExecutor::from_directory(
            root.clone(),
            directory.try_clone().map_err(failed)?,
            ReadLimits::default(),
            policy.clone(),
        )?;
        let mutation = write
            .then(|| MutationExecutor::from_directory(root, directory, self.writes.clone(), policy))
            .transpose()?;
        Ok(Box::pin(async move {
            if let File::Read(input) = input {
                return match read
                    .read(input.resolve().map_err(failed)?, cancellation)
                    .await?
                {
                    ReadOutput::Text(page) => {
                        Ok(Output::Value(serde_json::to_value(page).map_err(failed)?))
                    }
                    ReadOutput::Image { bytes, mime_type } => {
                        Ok(Output::Image { bytes, mime_type })
                    }
                };
            }
            let name = input.name().to_owned();
            let arguments = input.into_tool_input(&uuid::Uuid::new_v4().to_string());
            let executor: Arc<dyn ToolExecutor> = match mutation {
                Some(executor) => Arc::new(executor),
                None => Arc::new(read),
            };
            executor
                .invoke(name, arguments, cancellation)
                .await
                .map(Output::Value)
        }))
    }

    async fn prepare_resource_model(
        self: &Arc<Self>,
        boundary: Boundary,
        source: &maka_plugins::call::Identity,
        input: Generate,
        cancellation: CancellationToken,
    ) -> Result<Prepared<ModelGeneration>, ToolError> {
        input.validate().map_err(failed)?;
        let scope = match &boundary {
            Boundary::Session { boundary, .. } => {
                maka_plugins::composition::Scope::Session(boundary.session_id.clone())
            }
            _ => maka_plugins::composition::Scope::Profile,
        };
        let (key, model, thinking) = match boundary {
            Boundary::Session { boundary, .. } => {
                let session = self
                    .log
                    .get_session::<crate::session::SessionConfiguration>(&boundary.session_id)
                    .await
                    .map_err(failed)?
                    .ok_or_else(|| failed("Session unavailable"))?;
                (
                    boundary.session_id,
                    session.configuration.target.model().cloned(),
                    session.configuration.thinking_level,
                )
            }
            Boundary::Workspace { .. } => {
                // Provider affinity is an opaque request context, not a fabricated
                // canonical Session or an Agent invocation.
                let key = match source {
                    maka_plugins::call::Identity::Background { grant } => {
                        format!("plugin-grant:{}", grant.0)
                    }
                    maka_plugins::call::Identity::Remote { request_id } => {
                        format!("plugin-remote:{request_id}")
                    }
                    _ => return Err(failed("unexpected resource source")),
                };
                (key, None, None)
            }
            Boundary::Profile | Boundary::Directory { .. } => {
                return Err(failed("model generation requires a workspace"));
            }
        };
        let model = match model {
            Some(model) => model,
            None => crate::session::model::resolve(
                &self.configuration,
                &maka_protocol::session::SessionModelTarget::Default,
                thinking,
            )
            .await
            .map_err(|error| failed(error.message))?,
        };
        let provider =
            super::super::provider::observe_binding(&self.configuration, &key, &model, thinking)
                .await
                .map_err(|error| failed(error.message))?;
        let models = self.models.clone();
        let adapter = maka_model::adapters::resolve(
            &self.plugin_catalog.capture(&scope),
            provider.config.adapter_name(),
        )
        .map_err(failed)?;
        let host = self.clone();
        Ok(Prepared {
            operation: Operation::Model {
                input: input.clone(),
                model,
                thinking_level: thinking,
            },
            capability: Capability::Models,
            effect: Box::pin(async move {
                let provider = provider
                    .admit(&host.oauth)
                    .map_err(|error| failed(error.message))?;
                super::llm::generate(
                    models,
                    super::llm::request(provider, input),
                    adapter,
                    cancellation,
                )
                .await
            }),
        })
    }
}
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
