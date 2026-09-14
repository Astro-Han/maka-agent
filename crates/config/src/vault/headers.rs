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

use super::{advance, invalidate_test, write_secret};
use crate::{ConfigError, ConfigurationStore, Result, TransactionMode, catalog};
use maka_runtime::configuration::{headers::*, *};
use sqlx::SqliteConnection;
use std::collections::BTreeMap;

fn locator(id: String) -> CredentialLocator {
    CredentialLocator::Connection {
        connection_id: id,
        kind: ConnectionCredentialKind::RequestHeaders,
    }
}

async fn read(tx: &mut SqliteConnection, locator: &CredentialLocator) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT secret FROM credentials WHERE locator = ?")
        .bind(serde_json::to_string(locator)?)
        .fetch_optional(tx)
        .await
        .map_err(Into::into)
}

fn parse(secret: Option<&str>) -> Result<BTreeMap<String, String>> {
    validation::parse_headers(secret.unwrap_or("{}")).map_err(ConfigError::Invalid)
}

impl ConfigurationStore {
    pub async fn request_headers(&self, id: String) -> Result<RequestHeadersQueryResult> {
        validation::entity_id(&id).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move {
                if catalog::find(tx, &id).await?.is_none() {
                    return Ok(RequestHeadersQueryResult::ConnectionNotFound);
                }
                let secret = read(tx, &locator(id)).await?;
                Ok(RequestHeadersQueryResult::Found {
                    names: parse(secret.as_deref())?.into_keys().collect(),
                })
            })
        })
        .await
    }

    /// Complete replacement under the vault's existing transaction owner.
    /// Missing values retain that name's value; omitted names are deleted.
    pub async fn replace_request_headers(
        &self,
        mut input: RequestHeadersReplace,
        now: u64,
    ) -> Result<RequestHeadersReplaceResult> {
        input.normalize().map_err(ConfigError::Invalid)?;
        validation::revision(now, false).map_err(ConfigError::Invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| {
            Box::pin(async move {
                if catalog::find(tx, &input.connection_id).await?.is_none() {
                    return Ok(RequestHeadersReplaceResult::ConnectionNotFound);
                }
                let locator = locator(input.connection_id);
                let previous = read(tx, &locator).await?;
                let saved = parse(previous.as_deref())?;
                let by_name: BTreeMap<_, _> = saved
                    .iter()
                    .map(|(name, value)| (name.to_ascii_lowercase(), value))
                    .collect();
                let mut headers = BTreeMap::new();
                for update in input.headers {
                    let value =
                        match update.value {
                            Some(value) => value,
                            None => (*by_name.get(&update.name.to_ascii_lowercase()).ok_or_else(
                                || {
                                    ConfigError::Invalid(
                                        "new request header requires a value".into(),
                                    )
                                },
                            )?)
                            .clone(),
                        };
                    headers.insert(update.name, value);
                }
                // Retained values participate in the aggregate bound too.
                let secret = validation::normalize_headers(&serde_json::to_string(&headers)?)
                    .map_err(ConfigError::Invalid)?;
                let names = headers.keys().cloned().collect();
                if saved == headers && !(headers.is_empty() && previous.is_some()) {
                    return Ok(RequestHeadersReplaceResult::Unchanged { names });
                }
                if headers.is_empty() {
                    sqlx::query("DELETE FROM credentials WHERE locator = ?")
                        .bind(serde_json::to_string(&locator)?)
                        .execute(&mut *tx)
                        .await?;
                } else {
                    write_secret(tx, &locator, &secret, now).await?;
                }
                invalidate_test(tx, &locator).await?;
                advance(tx).await?;
                Ok(RequestHeadersReplaceResult::Committed { names })
            })
        })
        .await
    }
}
