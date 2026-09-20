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
mod changes;
mod request;
use self::request::Request;
use super::{ID, Skills};
use crate::api::{ImportSourceInput, InvocableTarget, WorkspaceContext};
use futures_util::future::BoxFuture;
use maka_plugins::{
    client::Bundle,
    contributions::Staged,
    remote::{Caller, Endpoint, Error, Handler, Method, WorkspaceViewInput, key},
};
use maka_runtime::execution::{CollaborationMode, PermissionMode, WorkspaceTarget};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Plugin-local wiring assembled from public Host capabilities.
#[derive(Clone)]
pub(super) struct ClientSupport {
    pub bundle: Arc<Bundle>,
}
pub const CLIENT_SERVICE: &str = "maka.skills.client";

#[derive(Clone, Copy)]
enum Source {
    Session,
    Project,
    Path,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectRequest {
    project_id: String,
    permission_mode: PermissionMode,
    collaboration_mode: CollaborationMode,
    request: Request,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PathRequest {
    path: String,
    permission_mode: PermissionMode,
    collaboration_mode: CollaborationMode,
    request: Request,
}

pub(super) fn publish(
    skills: &Skills,
    staged: &mut Staged,
    support: ClientSupport,
) -> Result<(), String> {
    staged
        .insert(
            key(ID, "changes").map_err(message)?,
            Endpoint::new(
                support.bundle.content_digest.clone(),
                Handler::Stream(Arc::new(changes::Provider(skills.changed.clone()))),
            ),
        )
        .map_err(message)?;
    for (name, source) in [
        ("request", Source::Session),
        ("project-request", Source::Project),
        ("path-request", Source::Path),
    ] {
        let endpoint = Endpoint::new(
            support.bundle.content_digest.clone(),
            Handler::Method(Arc::new(Service {
                skills: skills.clone(),
                source,
            })),
        );
        let endpoint = if matches!(source, Source::Path) {
            endpoint.requiring_host_paths()
        } else {
            endpoint
        };
        staged
            .insert(key(ID, name).map_err(message)?, endpoint)
            .map_err(message)?;
    }
    staged
        .insert(
            key(ID, "import-source").map_err(message)?,
            Endpoint::new(
                support.bundle.content_digest.clone(),
                Handler::Method(Arc::new(Import(skills.clone()))),
            )
            .requiring_host_paths(),
        )
        .map_err(message)
}
struct Service {
    skills: Skills,
    source: Source,
}
impl Method for Service {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let skills = self.skills.clone();
        let source = self.source;
        Box::pin(async move {
            let (request, view, target) = match source {
                Source::Session => {
                    let request = decode(input)?;
                    let session_id = caller
                        .session_id
                        .clone()
                        .ok_or_else(|| Error::Invalid("Skills requires a Session".into()))?;
                    let view = caller.views.session().await?;
                    (request, view, InvocableTarget::Session { session_id })
                }
                Source::Project | Source::Path => {
                    let (request, input) = match source {
                        Source::Project => {
                            let ProjectRequest {
                                project_id,
                                permission_mode,
                                collaboration_mode,
                                request,
                            } = decode(input)?;
                            (
                                request,
                                WorkspaceViewInput {
                                    workspace: WorkspaceTarget::Project { project_id },
                                    permission_mode,
                                    collaboration_mode,
                                },
                            )
                        }
                        Source::Path => {
                            let PathRequest {
                                path,
                                permission_mode,
                                collaboration_mode,
                                request,
                            } = decode(input)?;
                            (
                                request,
                                WorkspaceViewInput {
                                    workspace: WorkspaceTarget::HostPath { path },
                                    permission_mode,
                                    collaboration_mode,
                                },
                            )
                        }
                        Source::Session => unreachable!(),
                    };
                    let permission_mode = input.permission_mode;
                    let collaboration_mode = input.collaboration_mode;
                    let view = caller.views.workspace(input).await?;
                    let target = InvocableTarget::NewSession {
                        context: WorkspaceContext {
                            workspace: view.workspace.target.clone(),
                        },
                        permission_mode,
                        collaboration_mode,
                    };
                    (request, view, target)
                }
            };
            if caller.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            request.execute(&skills, view, target).await
        })
    }
}
struct Import(Skills);
impl Method for Import {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let skills = self.0.clone();
        Box::pin(async move {
            if caller.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let input: ImportSourceInput = decode(input)?;
            encode(skills.import_source(input).await.map_err(failure)?)
        })
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, Error> {
    serde_json::from_value(value).map_err(|error| Error::Invalid(error.to_string()))
}
fn encode(value: impl serde::Serialize) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|error| Error::Provider(error.to_string()))
}
fn message(error: impl ToString) -> String {
    error.to_string()
}
fn failure(error: super::Error) -> Error {
    match error {
        super::Error::Invalid(message) => Error::Invalid(message),
        super::Error::Retired => Error::Retired,
        super::Error::OutcomeUnknown(message) => Error::OutcomeUnknown(message),
        other => Error::Provider(other.to_string()),
    }
}
