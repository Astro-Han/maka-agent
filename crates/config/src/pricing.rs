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

mod catalog;
pub(crate) use catalog::initialize;

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode, database::unsigned};
use maka_runtime::pricing::{Entry, Mutation, Page, Pricing, Query, ResetEffect, Update, Updated};
use sqlx::{Row, SqliteConnection};

const MAX_REVISION: u64 = 9_007_199_254_740_991;
const PAGE_ITEMS: usize = 128;
// Reserve envelope space; each admitted record is bounded independently.
const PAGE_ENTRY_BYTES: usize = 47 * 1024;

impl ConfigurationStore {
    /// Continuations are valid only while both bundled and custom rates are unchanged.
    pub async fn query_pricing(&self, query: Query) -> Result<Page> {
        self.transaction(TransactionMode::Deferred, move |connection| Box::pin(async move {
            let revision = revision(connection).await?;
            let offset = match query {
                Query::Start => 0,
                Query::Continue { revision: expected, offset } => {
                    if expected > MAX_REVISION || offset > i64::MAX as u64 {
                        return Err(invalid("invalid pricing continuation"));
                    }
                    if expected != revision {
                        return Ok(Page::RevisionChanged { expected_revision: expected, actual_revision: revision });
                    }
                    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM effective_pricing")
                        .fetch_one(&mut *connection).await?;
                    if offset >= unsigned(count)? {
                        return Err(invalid("pricing offset is outside the catalog"));
                    }
                    offset
                }
            };
            let rows = sqlx::query(
                "SELECT record_json, custom, builtin FROM effective_pricing ORDER BY sort_key LIMIT ? OFFSET ?"
            ).bind((PAGE_ITEMS + 1) as i64).bind(offset as i64)
                .fetch_all(&mut *connection).await?;
            let mut entries = Vec::new();
            let mut bytes = 0;
            let mut more = false;
            for row in rows {
                let pricing = serde_json::from_str(row.try_get("record_json")?)?;
                let entry = if row.try_get::<bool, _>("custom")? {
                    Entry::Custom { pricing, reset_effect: if row.try_get::<bool, _>("builtin")? {
                        ResetEffect::RestoreBuiltin
                    } else { ResetEffect::BecomeUnpriced } }
                } else { Entry::Builtin { pricing } };
                let size = serde_json::to_vec(&entry)?.len() + 1;
                if entries.len() == PAGE_ITEMS || bytes + size > PAGE_ENTRY_BYTES {
                    more = true;
                    break;
                }
                bytes += size;
                entries.push(entry);
            }
            let next_offset = more.then_some(offset + entries.len() as u64);
            Ok(Page::Page { revision, offset, entries, next_offset })
        })).await
    }

    /// A quote is a value, not a live lookup. Persist it with the model accounting fact.
    pub async fn model_pricing(&self, model_key: String) -> Result<(u64, Option<Pricing>)> {
        maka_runtime::pricing::validate_key(&model_key).map_err(invalid)?;
        self.transaction(TransactionMode::Deferred, move |connection| {
            Box::pin(async move {
                let revision = revision(connection).await?;
                let record: Option<String> = sqlx::query_scalar(
                    "SELECT record_json FROM effective_pricing WHERE model_key = ?",
                )
                .bind(model_key)
                .fetch_optional(&mut *connection)
                .await?;
                Ok((
                    revision,
                    record
                        .map(|record| serde_json::from_str(&record))
                        .transpose()?,
                ))
            })
        })
        .await
    }

    pub async fn update_pricing(&self, update: Update) -> Result<Updated> {
        if update.expected_revision > MAX_REVISION {
            return Err(invalid("invalid pricing revision"));
        }
        match &update.mutation {
            Mutation::Upsert { pricing } => pricing.validate().map_err(invalid)?,
            Mutation::Delete { model_key } => {
                maka_runtime::pricing::validate_key(model_key).map_err(invalid)?
            }
        }
        self.transaction(TransactionMode::Immediate, move |connection| Box::pin(async move {
            let current = revision(connection).await?;
            if current != update.expected_revision {
                return Ok(Updated::RevisionConflict {
                    expected_revision: update.expected_revision, actual_revision: current,
                });
            }
            let changed = match update.mutation {
                Mutation::Upsert { pricing } => {
                    let old: Option<String> = sqlx::query_scalar(
                        "SELECT record_json FROM pricing_overrides WHERE model_key = ?"
                    ).bind(&pricing.model_key).fetch_optional(&mut *connection).await?;
                    if old.as_deref().map(serde_json::from_str::<Pricing>).transpose()?.as_ref() == Some(&pricing) {
                        false
                    } else {
                        let record = serde_json::to_string(&pricing)?;
                        if record.len() > 4096 { return Err(invalid("model price exceeds record limit")); }
                        sqlx::query(
                            "INSERT INTO pricing_overrides(model_key, sort_key, record_json) VALUES(?, ?, ?)
                             ON CONFLICT(model_key) DO UPDATE SET record_json = excluded.record_json"
                        ).bind(&pricing.model_key).bind(sort_key(&pricing.model_key)).bind(record)
                            .execute(&mut *connection).await?;
                        true
                    }
                }
                Mutation::Delete { model_key } => sqlx::query(
                    "DELETE FROM pricing_overrides WHERE model_key = ?"
                ).bind(model_key).execute(&mut *connection).await?.rows_affected() != 0,
            };
            if !changed { return Ok(Updated::Unchanged { revision: current }); }
            let revision = current.checked_add(1).filter(|revision| *revision <= MAX_REVISION)
                .ok_or_else(|| invalid("pricing revision exhausted"))?;
            sqlx::query("UPDATE pricing_authority SET revision = ? WHERE singleton = 1")
                .bind(revision as i64).execute(&mut *connection).await?;
            Ok(Updated::Committed { revision })
        })).await
    }
}

async fn revision(connection: &mut SqliteConnection) -> Result<u64> {
    unsigned(
        sqlx::query_scalar("SELECT revision FROM pricing_authority WHERE singleton = 1")
            .fetch_one(connection)
            .await?,
    )
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

// Big-endian UTF-16 preserves the client's exact, locale-independent key ordering.
fn sort_key(key: &str) -> Vec<u8> {
    key.encode_utf16().flat_map(u16::to_be_bytes).collect()
}
