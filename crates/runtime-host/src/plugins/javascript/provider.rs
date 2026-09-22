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

use super::{
    callbacks::{Callback, invoke_with_grace},
    model::Calls,
};
use futures_util::future::BoxFuture;
use maka_plugins::{
    model::Credentials,
    provider::{
        Connection, Context, Error, Model, Provider, Resolve,
        authentication::{Authenticate, Credential},
    },
};
use maka_runtime::configuration::ModelInfo;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) struct JavaScript {
    pub callback: Arc<Callback>,
    pub calls: Arc<Calls>,
}

#[derive(Serialize)]
#[serde(
    tag = "method",
    content = "input",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Operation {
    Resolve(Resolve),
    Authorize {
        connection: Connection,
        credential: Option<Credential>,
        session_id: String,
    },
    Authenticate(Authenticate),
    Refresh {
        connection: Connection,
        credential: Credential,
    },
    Discover {
        connection: Connection,
        credential: Option<Credential>,
    },
}

impl JavaScript {
    async fn invoke<T: DeserializeOwned>(
        &self,
        operation: Operation,
        context: Option<Context>,
    ) -> Result<T, Error> {
        let exchanging = matches!(
            operation,
            Operation::Authenticate(_) | Operation::Refresh { .. }
        );
        let deadline = std::time::Duration::from_secs(match &operation {
            Operation::Authenticate(_) => 15 * 60,
            Operation::Refresh { .. } | Operation::Discover { .. } => 30,
            Operation::Resolve(_) | Operation::Authorize { .. } => 10,
        });
        let input = serde_json::to_value(operation)
            .map_err(|_| Error::Invalid("invalid provider input".into()))?;
        let cancellation = context
            .as_ref()
            .map(|c| c.cancellation.child_token())
            .unwrap_or_default();
        let routing = context.as_ref().map(|c| c.transport.identity().to_string());
        let handle = context
            .map(|c| self.calls.provider(c))
            .transpose()
            .map_err(|_| Error::Unavailable)?;
        let call = match &handle {
            Some(handle) => json!({"model": handle.id, "routing": routing, "provider": true}),
            None => Value::Null,
        };
        let invocation = invoke_with_grace(
            &self.callback.module,
            self.callback.id,
            input,
            call,
            cancellation.clone(),
            std::time::Duration::from_secs(if exchanging { 35 } else { 5 }),
        );
        tokio::pin!(invocation);
        let result = tokio::select! {
            result = &mut invocation => result,
            _ = tokio::time::sleep(deadline) => {
                cancellation.cancel();
                // The callback still owns a potentially spent grant. Drain it;
                // dropping the waiting UI is never a refresh rollback.
                invocation.await
            }
        }
        .map_err(|_| {
            if exchanging {
                Error::OutcomeUnknown
            } else {
                Error::Unavailable
            }
        })?;
        if let Some(error) = result.get("error") {
            return Err(serde_json::from_value(error.clone())
                .map_err(|_| Error::Invalid("invalid provider failure".into()))?);
        }
        serde_json::from_value(
            result
                .get("value")
                .cloned()
                .ok_or_else(|| Error::Invalid("missing provider result".into()))?,
        )
        .map_err(|_| Error::Invalid("invalid provider result".into()))
    }
}
impl Provider for JavaScript {
    fn resolve(&self, request: Resolve) -> BoxFuture<'_, Result<Model, Error>> {
        Box::pin(async move {
            let model: Model = self.invoke(Operation::Resolve(request), None).await?;
            model.validate()?;
            Ok(model)
        })
    }
    fn authorize(
        &self,
        connection: Connection,
        credential: Option<Credential>,
        session_id: String,
    ) -> BoxFuture<'_, Result<Credentials, Error>> {
        Box::pin(self.invoke(
            Operation::Authorize {
                connection,
                credential,
                session_id,
            },
            None,
        ))
    }
    fn authenticate(
        &self,
        request: Authenticate,
        context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async move {
            let credential: Credential = self
                .invoke(Operation::Authenticate(request), Some(context))
                .await?;
            credential.validate()?;
            Ok(credential)
        })
    }
    fn refresh(
        &self,
        connection: Connection,
        credential: Credential,
        context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async move {
            let credential: Credential = self
                .invoke(
                    Operation::Refresh {
                        connection,
                        credential,
                    },
                    Some(context),
                )
                .await?;
            credential.validate()?;
            Ok(credential)
        })
    }
    fn discover(
        &self,
        connection: Connection,
        credential: Option<Credential>,
        context: Context,
    ) -> BoxFuture<'_, Result<Vec<ModelInfo>, Error>> {
        Box::pin(async move {
            let models: Vec<ModelInfo> = self
                .invoke(
                    Operation::Discover {
                        connection,
                        credential,
                    },
                    Some(context),
                )
                .await?;
            if models.len() > 2048 {
                return Err(Error::Invalid(
                    "model inventory exceeds 2048 entries".into(),
                ));
            }
            for model in &models {
                model.validate().map_err(Error::Invalid)?;
            }
            Ok(models)
        })
    }
}
