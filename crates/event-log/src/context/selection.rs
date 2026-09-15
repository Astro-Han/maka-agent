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

//! A disposable SQL selection derived from immutable openings, never an authority or cache.
use crate::{StoreError, sequence_number};
use maka_runtime::event::{Fact, Invocation, InvocationInput, LogScope, RuntimeEvent};
use sqlx::SqliteConnection;

pub(crate) struct Selection {
    pub scope: LogScope,
    pub session: Option<String>,
    pub lineage: Option<String>,
}

impl Selection {
    pub fn session(session: &str) -> Self {
        Self {
            scope: LogScope::Session { id: session.into() },
            session: Some(session.into()),
            lineage: None,
        }
    }

    pub async fn resolve(
        connection: &mut SqliteConnection,
        scope: &LogScope,
    ) -> Result<Self, StoreError> {
        let (session, run) = match scope {
            LogScope::Root => {
                return Ok(Self {
                    scope: scope.clone(),
                    session: None,
                    lineage: None,
                });
            }
            LogScope::Session { id } => {
                crate::sessions::validate_id(id)?;
                return Ok(Self::session(id));
            }
            LogScope::Lineage { session_id, run_id } => (session_id, run_id),
        };
        crate::sessions::validate_id(session)?;
        crate::sessions::validate_id(run)?;
        let invocation = crate::turns::run_invocation(connection, session, run)
            .await?
            .ok_or_else(|| super::invalid("lineage root Run is missing"))?;
        let (sequence, event) =
            super::proof::by_kind(connection, &invocation, "invocation_opened", None).await?;
        if event.invocation.session_id != *session || event.invocation.run_id != *run {
            return Err(super::invalid("lineage root identity changed"));
        }
        let Fact::InvocationOpened {
            input,
            configuration,
        } = event.fact
        else {
            return Err(super::invalid("lineage root has no opening"));
        };
        let mut runs = vec![invocation];
        let base = match &input {
            InvocationInput::Message { .. } => {
                super::evidence::high_water(connection, session, sequence as i64).await?
            }
            InvocationInput::Continuation { claim, .. }
            | InvocationInput::Handoff { claim, .. } => {
                input
                    .validate_inheritance(&event.invocation)
                    .map_err(super::invalid)?;
                let workspace = configuration
                    .as_ref()
                    .and_then(|c| c.workspace_identity.as_ref())
                    .ok_or_else(|| super::invalid("lineage root has no workspace identity"))?;
                runs.extend(
                    crate::continuation::ancestors(
                        connection,
                        &claim.source,
                        &claim.base,
                        workspace,
                    )
                    .await?
                    .into_iter()
                    .map(|i| i.invocation_id),
                );
                claim.base.high_water
            }
            _ => return Err(super::invalid("opening does not inherit model history")),
        };
        Ok(Self {
            scope: scope.clone(),
            session: Some(session.clone()),
            lineage: Some(serde_json::to_string(
                &serde_json::json!({"base":base,"runs":runs}),
            )?),
        })
    }

    /// The canonical opening authenticates selection independently of the read cut.
    /// In particular, pre-turn compaction reads before this opening.
    pub async fn for_opening(
        connection: &mut SqliteConnection,
        opening: &RuntimeEvent,
    ) -> Result<Self, StoreError> {
        let Fact::InvocationOpened { input, .. } = &opening.fact else {
            return Err(super::invalid("selection requires a canonical opening"));
        };
        if input.inherited_claim().is_some() {
            Self::resolve(
                connection,
                &LogScope::Lineage {
                    session_id: opening.invocation.session_id.clone(),
                    run_id: opening.invocation.run_id.clone(),
                },
            )
            .await
        } else {
            Ok(Self::session(&opening.invocation.session_id))
        }
    }

    pub async fn for_invocation(
        connection: &mut SqliteConnection,
        invocation: &Invocation,
    ) -> Result<Self, StoreError> {
        let (_, opening) = super::proof::by_kind(
            connection,
            &invocation.invocation_id,
            "invocation_opened",
            None,
        )
        .await?;
        if opening.invocation != *invocation {
            return Err(super::invalid("selection opening identity changed"));
        }
        Self::for_opening(connection, &opening).await
    }

    /// Alias and parameter are internal SQL identifiers, never client values.
    pub fn predicate(alias: &str, parameter: &str) -> String {
        format!(
            "({parameter} IS NULL OR {alias}.sequence <= json_extract({parameter},'$.base') OR {alias}.invocation_id IN (SELECT value FROM json_each({parameter},'$.runs')))"
        )
    }

    pub async fn high_water(
        &self,
        connection: &mut SqliteConnection,
        through: u64,
    ) -> Result<u64, StoreError> {
        let filter = Self::predicate("e", "?3");
        let session = self
            .session
            .as_deref()
            .ok_or_else(|| super::invalid("context requires a Session"))?;
        sequence_number(
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COALESCE(MAX(e.sequence),0) FROM runtime_events e WHERE e.sequence <= ?1
             AND json_extract(e.event_json,'$.invocation.session_id')=?2 AND {filter}"
            )))
            .bind(through as i64)
            .bind(session)
            .bind(&self.lineage)
            .fetch_one(connection)
            .await?,
        )
    }
}
