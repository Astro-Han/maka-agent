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

//! Disposable bundled-rate projection. Only overrides and the revision are durable.
use super::{MAX_REVISION, sort_key};
use maka_event_log::StoreError;
use maka_runtime::pricing::Pricing;
use sha2::{Digest, Sha256};
use sqlx::{Connection, QueryBuilder, SqliteConnection};
use std::sync::LazyLock;

const FACTS: &str = include_str!("../../data/pricing-facts.json");
static DIGEST: LazyLock<String> = LazyLock::new(|| format!("{:x}", Sha256::digest(FACTS)));
static PRICES: LazyLock<Vec<Pricing>> = LazyLock::new(|| {
    let prices: Vec<Pricing> = serde_json::from_str(FACTS).expect("generated model rates");
    for price in &prices {
        price.validate().expect("valid generated model rate");
    }
    prices
});

pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::raw_sql(
        "CREATE TEMP TABLE builtin_pricing (
            model_key TEXT PRIMARY KEY, sort_key BLOB NOT NULL, record_json TEXT NOT NULL
        );
        CREATE TEMP VIEW effective_pricing AS
            SELECT b.model_key, b.sort_key, b.record_json, 0 AS custom, 1 AS builtin
            FROM builtin_pricing b WHERE NOT EXISTS (
                SELECT 1 FROM main.pricing_overrides p WHERE p.model_key = b.model_key
            )
            UNION ALL
            SELECT p.model_key, p.sort_key, p.record_json, 1 AS custom,
                EXISTS(SELECT 1 FROM builtin_pricing b WHERE b.model_key = p.model_key) AS builtin
            FROM main.pricing_overrides p;",
    )
    .execute(&mut *tx)
    .await?;
    for chunk in PRICES.chunks(128) {
        let mut query =
            QueryBuilder::new("INSERT INTO builtin_pricing(model_key, sort_key, record_json) ");
        query.push_values(chunk, |mut row, pricing| {
            row.push_bind(&pricing.model_key)
                .push_bind(sort_key(&pricing.model_key))
                .push_bind(serde_json::to_string(pricing).expect("finite generated rates"));
        });
        query.build().execute(&mut *tx).await?;
    }
    // An application update can change bundled rates without changing overrides.
    // Invalidate previously issued cursors and distinguish newly captured quotes.
    let changed = sqlx::query(
        "UPDATE pricing_authority
         SET revision = revision + CASE WHEN builtin_digest IS NULL THEN 0 ELSE 1 END,
             builtin_digest = ?
         WHERE singleton = 1 AND builtin_digest IS NOT ?
           AND (builtin_digest IS NULL OR revision < ?)",
    )
    .bind(DIGEST.as_str())
    .bind(DIGEST.as_str())
    .bind(MAX_REVISION as i64)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        let digest: String =
            sqlx::query_scalar("SELECT builtin_digest FROM pricing_authority WHERE singleton = 1")
                .fetch_one(&mut *tx)
                .await?;
        if digest != *DIGEST {
            return Err(StoreError::InvalidTransition(
                "pricing revision exhausted".into(),
            ));
        }
    }
    tx.commit().await?;
    Ok(())
}
