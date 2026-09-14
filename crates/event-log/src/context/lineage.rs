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

use super::{ModelContextSource, frozen, invalid, selection::Selection};
use crate::EventLog;
use crate::StoreError;
use maka_runtime::{
    continuation::RunBoundary,
    event::{Fact, LogScope},
};
use sqlx::Connection;

impl EventLog {
    /// Reconstruct a sealed source's selected history, not the current whole Session.
    /// The raw boundary and each inherited edge are authenticated in the same SQL snapshot.
    pub async fn read_lineage_context(
        &self,
        source: &RunBoundary,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<ModelContextSource, StoreError> {
        self.validate_root()?;
        for id in [
            &source.invocation.session_id,
            &source.invocation.turn_id,
            &source.invocation.run_id,
            &source.invocation.invocation_id,
        ] {
            crate::sessions::validate_id(id)?;
        }
        if source.high_water == 0 || source.high_water > 9_007_199_254_740_991 {
            return Err(invalid("invalid source Run high-water"));
        }
        let source = source.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let invocation = &source.invocation;
                    let id = crate::turns::run_invocation(
                        &mut tx,
                        &invocation.session_id,
                        &invocation.run_id,
                    )
                    .await?
                    .ok_or_else(|| invalid("lineage source Run is missing"))?;
                    if id != invocation.invocation_id {
                        return Err(invalid("lineage source identity changed"));
                    }
                    let through: i64 = sqlx::query_scalar(
                        "SELECT MAX(sequence) FROM runtime_events WHERE invocation_id=?",
                    )
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await?;
                    let prefix = crate::run_prefix::read(
                        &mut tx,
                        &invocation.session_id,
                        &invocation.run_id,
                        &id,
                        through,
                        10_000,
                        8 * 1024 * 1024,
                    )
                    .await?;
                    if prefix.invocation != *invocation
                        || prefix.high_water != source.high_water
                        || prefix.digest != source.digest
                        || !matches!(
                            prefix.events.last().map(|e| &e.event.fact),
                            Some(Fact::InvocationEnded { .. })
                        )
                    {
                        return Err(invalid("lineage source is not its exact sealed boundary"));
                    }
                    let scope = LogScope::Lineage {
                        session_id: invocation.session_id.clone(),
                        run_id: invocation.run_id.clone(),
                    };
                    let selection = Selection::resolve(&mut tx, &scope).await?;
                    let context = frozen::closed_selected(
                        &mut tx,
                        &selection,
                        through as u64,
                        max_events,
                        max_bytes,
                    )
                    .await?;
                    tx.commit().await?;
                    Ok(context)
                })
            })
            .await
    }
}
