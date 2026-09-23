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

use crate::preferences;
use futures_util::future::BoxFuture;
use maka_plugins::{
    authorization::{Capability, Request as Authorization, Target},
    execution::CommandError,
    pricing,
    remote::{Caller, Error, Method},
    storage, usage,
};
use maka_runtime::tools::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, sync::Arc};

#[derive(Clone)]
pub(super) struct Insights {
    pub usage: Arc<dyn usage::Usage>,
    pub pricing: Arc<dyn pricing::Prices>,
    pub store: Arc<dyn storage::Store>,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Request {
    Preferences,
    SavePreferences {
        expected_revision: Option<u64>,
        preferences: preferences::Preferences,
    },
    Activity {
        operation_id: uuid::Uuid,
        read: usage::Read,
    },
    Summary {
        operation_id: uuid::Uuid,
        cursor: String,
    },
    Prices {
        query: pricing::Query,
    },
    UpdatePrice {
        operation_id: uuid::Uuid,
        update: pricing::Update,
    },
}
#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Response {
    Preferences { snapshot: preferences::Snapshot },
    Activity { page: usage::Page },
    Summary { summary: Box<usage::Summary> },
    Prices { page: pricing::Page },
    PriceUpdated { receipt: pricing::Updated },
    RefreshRequired,
}

impl Method for Insights {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let insights = self.clone();
        Box::pin(async move {
            let request: Request =
                serde_json::from_value(input).map_err(|error| Error::Invalid(error.to_string()))?;
            let response = match request {
                Request::Preferences => Response::Preferences {
                    snapshot: preferences::read(insights.store.as_ref())
                        .await
                        .map_err(store_error)?,
                },
                Request::SavePreferences {
                    expected_revision,
                    preferences,
                } => {
                    preferences
                        .selection
                        .validate()
                        .map_err(|message| Error::Invalid(message.into()))?;
                    match preferences::write(
                        insights.store.as_ref(),
                        expected_revision,
                        preferences,
                    )
                    .await
                    {
                        Ok(snapshot) => Response::Preferences { snapshot },
                        Err(storage::StoreError::Conflict { .. }) => Response::RefreshRequired,
                        Err(error) => return Err(store_error(error)),
                    }
                }
                Request::Activity { operation_id, read } => {
                    authorized(
                        &caller,
                        operation_id,
                        Capability::ReadUsage,
                        |scope| async move {
                            match insights.usage.activity(scope, read).await {
                                Ok(page) => Ok(Response::Activity { page }),
                                Err(error) => command_error(error),
                            }
                        },
                    )
                    .await?
                }
                Request::Summary {
                    operation_id,
                    cursor,
                } => {
                    authorized(
                        &caller,
                        operation_id,
                        Capability::ReadUsage,
                        |scope| async move {
                            match insights.usage.summary(scope, cursor).await {
                                Ok(summary) => Ok(Response::Summary {
                                    summary: Box::new(summary),
                                }),
                                Err(error) => command_error(error),
                            }
                        },
                    )
                    .await?
                }
                Request::Prices { query } => match insights.pricing.query(query).await {
                    Ok(page) => Response::Prices { page },
                    Err(error) => command_error(error)?,
                },
                Request::UpdatePrice {
                    operation_id,
                    update,
                } => {
                    authorized(
                        &caller,
                        operation_id,
                        Capability::ManagePricing,
                        |scope| async move {
                            let receipt = insights
                                .pricing
                                .update(scope, update)
                                .await
                                .map_err(settlement)?;
                            Ok(Response::PriceUpdated { receipt })
                        },
                    )
                    .await?
                }
            };
            serde_json::to_value(response).map_err(|error| Error::Provider(error.to_string()))
        })
    }
}

/// Actual user Remote authority; it is neither an Agent invocation nor a stored background grant.
async fn authorized<T, F, Fut>(
    caller: &Caller,
    operation_id: uuid::Uuid,
    capability: Capability,
    operation: F,
) -> Result<T, Error>
where
    F: FnOnce(maka_plugins::call::Scope) -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let owned = caller
        .views
        .authorize(Authorization {
            operation_id,
            title: match capability {
                Capability::ManagePricing => "Change model pricing",
                _ => "Read usage accounting",
            }
            .into(),
            target: match (&caller.session_id, capability) {
                (Some(session_id), Capability::ReadUsage) => Target::Session {
                    session_id: session_id.clone(),
                },
                _ => Target::Profile,
            },
            capabilities: [capability].into(),
        })
        .await?;
    let result = operation(owned.scope()).await;
    owned.finish().await.map_err(settlement)?;
    result
}
fn command_error(error: CommandError) -> Result<Response, Error> {
    match error {
        CommandError::Conflict => Ok(Response::RefreshRequired),
        CommandError::Denied | CommandError::Revoked | CommandError::Draining => {
            Err(Error::Retired)
        }
        CommandError::Invalid(message) => Err(Error::Invalid(message)),
        other => Err(Error::Provider(other.to_string())),
    }
}
fn store_error(error: storage::StoreError) -> Error {
    match error {
        storage::StoreError::Retired => Error::Retired,
        storage::StoreError::OutcomeUnknown(message) => Error::OutcomeUnknown(message),
        other => Error::Provider(other.to_string()),
    }
}
fn settlement(error: ToolError) -> Error {
    match error {
        ToolError::Persistence(message) | ToolError::OutcomeUnknown(message) => {
            Error::OutcomeUnknown(message)
        }
        ToolError::CleanupUnconfirmed(_) => Error::CleanupUnconfirmed,
        other => Error::Provider(other.to_string()),
    }
}
