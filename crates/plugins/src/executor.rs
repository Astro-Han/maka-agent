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

use crate::{contributions::Contribution, fiber::CallGuard};
use futures_util::future::BoxFuture;
use maka_runtime::{
    event::Invocation,
    executor::{ExecutorId, Output},
    input::MessageInput,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub tool_activity: bool,
    #[serde(default)]
    pub attachments: bool,
}

#[derive(Clone)]
pub struct Request {
    pub invocation: Invocation,
    pub conversation_key: String,
    pub content: MessageInput,
    pub cwd: String,
    pub instructions: Option<String>,
    pub settings: maka_runtime::executor::Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Completed {
        text: String,
    },
    Cancelled {
        reason: Option<String>,
    },
    Failed {
        message: String,
        code: Option<String>,
        #[serde(default)]
        recoverable: bool,
    },
}
impl Outcome {
    fn validate(&self) -> Result<(), Error> {
        let size = serde_json::to_vec(self)
            .map_err(|error| Error::Invalid(error.to_string()))?
            .len();
        if size > 1024 * 1024 {
            return Err(Error::Invalid("executor result exceeds 1 MiB".into()));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("executor registration retired")]
    Retired,
    #[error("execution cancelled by caller")]
    Cancelled,
    #[error("executor did not settle after cancellation")]
    CleanupUnconfirmed,
    #[error("invalid executor input or output: {0}")]
    Invalid(String),
    #[error("executor output persistence failed: {0}")]
    Persistence(String),
    #[error("executor failed: {0}")]
    Provider(String),
}

/// Resolves only after the observation is committed. Consumer loss does not
/// undo accepted execution or permit unbounded output buffering.
pub trait OutputSink: Send + Sync {
    fn emit(&self, output: Output) -> BoxFuture<'_, Result<(), Error>>;
}
#[derive(Clone)]
pub struct Context {
    pub cancellation: CancellationToken,
    pub output: Arc<dyn OutputSink>,
    /// Issued by the embedding Host after admission, never reconstructed from
    /// Request.invocation. Absent in embedders without system capabilities.
    pub call: Option<crate::call::Scope>,
}
pub trait Provider: Send + Sync {
    fn execute(
        &self,
        request: Request,
        context: Context,
    ) -> BoxFuture<'static, Result<Outcome, Error>>;
}

pub struct Executor {
    pub id: ExecutorId,
    pub display_name: String,
    pub capabilities: Capabilities,
    pub provider: Arc<dyn Provider>,
}

pub struct Binding {
    session: String,
    contribution: Contribution<Executor>,
    calls: Option<crate::call::Issuer>,
}
pub struct Call {
    request: Request,
    contribution: Contribution<Executor>,
    lease: CallGuard,
    calls: Option<crate::call::Issuer>,
}
/// Keep this value alive until the Host commits the terminal execution fact.
pub struct Settlement {
    pub result: Result<Outcome, Error>,
    _lease: CallGuard,
}
impl Binding {
    pub fn with_calls(mut self, calls: crate::call::Issuer) -> Self {
        self.calls = Some(calls);
        self
    }
    pub fn is_effective(&self) -> bool {
        self.contribution.is_effective()
    }
    pub fn identity(&self) -> Result<maka_runtime::executor::Binding, Error> {
        let identity = self
            .contribution
            .owner
            .identity()
            .map_err(|_| Error::Retired)?;
        Ok(maka_runtime::executor::Binding {
            executor_id: self.contribution.value.id.clone(),
            package_id: identity.package_id,
            entry_id: identity.entry_id,
            activation: identity.activation,
        })
    }
    pub fn new(session: String, contribution: Contribution<Executor>) -> Result<Self, Error> {
        maka_runtime::interaction::entity_id(&session)
            .map_err(|error| Error::Invalid(error.into()))?;
        if contribution
            .owner
            .identity()
            .map_err(|_| Error::Retired)?
            .scope
            == crate::composition::Scope::DesktopUi
        {
            return Err(Error::Invalid("desktop-ui cannot provide executors".into()));
        }
        if let crate::composition::Scope::Session(scope) = contribution
            .owner
            .identity()
            .map_err(|_| Error::Retired)?
            .scope
            && scope != session
        {
            return Err(Error::Invalid("executor belongs to another Session".into()));
        }
        Ok(Self {
            session,
            contribution,
            calls: None,
        })
    }
    pub fn admit(&self, request: Request) -> Result<Call, Error> {
        request
            .settings
            .validate()
            .map_err(|error| Error::Invalid(error.into()))?;
        if request.invocation.session_id != self.session
            || request.conversation_key.is_empty()
            || request.conversation_key.len() > 256
            || request.content.text_bytes() > 64 * 1024
            || !std::path::Path::new(&request.cwd).is_absolute()
            || request.cwd.len() > 4096
            || request.cwd.contains('\0')
            || request
                .instructions
                .as_ref()
                .is_some_and(|text| text.len() > 64 * 1024)
        {
            return Err(Error::Invalid(
                "invalid or cross-Session execution request".into(),
            ));
        }
        if !self.contribution.value.capabilities.attachments
            && request
                .content
                .attachments
                .as_ref()
                .is_some_and(|items| !items.is_empty())
        {
            return Err(Error::Invalid(
                "executor does not support attachments".into(),
            ));
        }
        let lease = self.contribution.admit().map_err(|_| Error::Retired)?;
        Ok(Call {
            request,
            contribution: self.contribution.clone(),
            lease,
            calls: self.calls.clone(),
        })
    }
}
impl Call {
    pub async fn execute(
        self,
        output: Arc<dyn OutputSink>,
        cancellation: CancellationToken,
    ) -> Settlement {
        let token = cancellation.child_token();
        let closed = token.clone().drop_guard();
        let admitted = match self.calls.as_ref() {
            Some(issuer) => tokio::select! {
                biased;
                _ = token.cancelled() => Err(Error::Cancelled),
                _ = self.contribution.retired() => Err(Error::Retired),
                result = issuer.admit(
                    crate::call::Identity::Agent {
                        invocation: self.request.invocation.clone(),
                        operation_id: None,
                    },
                    token.clone(),
                ) => result.map(Some).map_err(|error| match error {
                    maka_runtime::tools::ToolError::Persistence(message) => Error::Persistence(message),
                    maka_runtime::tools::ToolError::CleanupUnconfirmed(_) => Error::CleanupUnconfirmed,
                    _ if token.is_cancelled() => Error::Cancelled,
                    other => Error::Provider(other.to_string()),
                }),
            },
            None => Ok(None),
        };
        let scope = match admitted {
            Ok(scope) => scope,
            Err(error) => {
                return Settlement {
                    result: Err(error),
                    _lease: self.lease,
                };
            }
        };
        let context = Context {
            call: scope.clone(),
            cancellation: token.clone(),
            output: Arc::new(CheckedOutput {
                sink: output,
                cancellation: token.clone(),
                capabilities: self.contribution.value.capabilities,
            }),
        };
        let mut execution = self
            .contribution
            .value
            .provider
            .execute(self.request, context);
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(Error::Cancelled),
            _ = self.contribution.retired() => Err(Error::Retired),
            result = &mut execution => {
                return Settlement {
                    result: settle(result, &self.contribution.owner, scope.as_ref()).await,
                    _lease: self.lease,
                };
            },
        };
        token.cancel();
        // Signalling precedes waiting. The Fiber independently signals owned
        // processes, so cleanup never depends on execute returning first.
        let result = match tokio::time::timeout(Duration::from_secs(5), execution).await {
            Ok(Err(Error::CleanupUnconfirmed)) => {
                self.contribution
                    .owner
                    .cleanup_failed("executor cleanup is unconfirmed".into());
                Err(Error::CleanupUnconfirmed)
            }
            Ok(_) => result,
            Err(_) => {
                self.contribution
                    .owner
                    .cleanup_failed("executor ignored cancellation; cleanup is unconfirmed".into());
                Err(Error::CleanupUnconfirmed)
            }
        };
        drop(closed);
        Settlement {
            result: settle(result, &self.contribution.owner, scope.as_ref()).await,
            _lease: self.lease,
        }
    }
}
async fn settle(
    mut result: Result<Outcome, Error>,
    owner: &crate::fiber::Context,
    scope: Option<&crate::call::Scope>,
) -> Result<Outcome, Error> {
    if let Some(scope) = scope
        && scope.finish().await.is_err()
    {
        result = Err(Error::CleanupUnconfirmed);
    }
    if owner.cleanup_failure().is_some() {
        result = Err(Error::CleanupUnconfirmed);
    }
    if matches!(result, Err(Error::CleanupUnconfirmed)) {
        owner.cleanup_failed("executor cleanup is unconfirmed".into());
    }
    result.and_then(|outcome| {
        outcome.validate()?;
        Ok(outcome)
    })
}
struct CheckedOutput {
    sink: Arc<dyn OutputSink>,
    cancellation: CancellationToken,
    capabilities: Capabilities,
}
impl OutputSink for CheckedOutput {
    fn emit(&self, output: Output) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if self.cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            output
                .validate()
                .map_err(|error| Error::Invalid(error.into()))?;
            match &output {
                Output::ThinkingDelta { .. } if !self.capabilities.thinking => {
                    return Err(Error::Invalid(
                        "executor did not declare thinking output".into(),
                    ));
                }
                Output::ToolStart { .. }
                | Output::ToolProgress { .. }
                | Output::ToolResult { .. }
                    if !self.capabilities.tool_activity =>
                {
                    return Err(Error::Invalid(
                        "executor did not declare tool activity".into(),
                    ));
                }
                _ => {}
            }
            self.sink.emit(output).await
        })
    }
}
