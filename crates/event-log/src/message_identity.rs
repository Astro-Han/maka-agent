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

use crate::{EventLog, StoreError};
use maka_runtime::{
    event::{Fact, RuntimeEvent},
    tool_call::tool_use_id,
};
use sqlx::SqliteConnection;

impl EventLog {
    /// Admission preflight only. The canonical INSERT checks the same namespace
    /// again in its transaction; this does not claim or reserve the identity.
    pub async fn validate_message_identity(
        &self,
        session: &str,
        message: &str,
    ) -> Result<(), StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        crate::sessions::validate_id(message)?;
        let session = session.to_owned();
        let message = message.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move { validate_source_id(connection, &session, &message).await })
            })
            .await
    }
}

/// Source IDs share the client's visible-message namespace with Host event and
/// tool IDs. Check both directions before either side becomes a durable fact.
pub(crate) async fn validate(
    tx: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    let session = &event.invocation.session_id;
    // All event IDs are checked by the INSERT itself, without an extra query
    // for every streamed delta. Derived tool IDs are checked here.
    let invocation = &event.invocation.invocation_id;
    match &event.fact {
        Fact::ModelCompleted { step_id, output } => {
            for (index, part) in output.parts.iter().enumerate() {
                if matches!(part, maka_runtime::model::ModelPart::ToolResult { .. }) {
                    reject_claim(
                        tx,
                        session,
                        &maka_runtime::tool_call::provider_result_id(&event.id, index),
                    )
                    .await?;
                }
            }
            for call in output.tool_calls() {
                reject_claim(
                    tx,
                    session,
                    &tool_use_id(invocation, &format!("{step_id}:{}", call.id)),
                )
                .await?;
            }
        }
        Fact::ToolDispatched { operation_id, .. } | Fact::ToolRejected { operation_id, .. } => {
            reject_claim(tx, session, &tool_use_id(invocation, operation_id)).await?;
        }
        Fact::ToolSettled { .. } => {
            reject_claim(
                tx,
                session,
                &maka_runtime::tool_call::metered_usage_id(&event.id),
            )
            .await?;
        }
        _ => {}
    }
    for message_id in crate::message_sources::identities(event) {
        validate_source_id(tx, session, message_id).await?;
    }
    Ok(())
}
pub(crate) async fn validate_source_id(
    tx: &mut SqliteConnection,
    session: &str,
    message_id: &str,
) -> Result<(), StoreError> {
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE event_id = ?
             AND json_extract(event_json, '$.invocation.session_id') = ?)",
    )
    .bind(message_id)
    .bind(session)
    .fetch_one(&mut *tx)
    .await?;
    if used {
        return Err(conflict());
    }
    if let Some(event_id) = maka_runtime::tool_call::parse_metered_usage_id(message_id) {
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE event_id = ?1
                 AND kind = 'tool_settled'
                 AND json_extract(event_json, '$.invocation.session_id') = ?2)",
        )
        .bind(event_id)
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
        if used {
            return Err(conflict());
        }
    }
    if let Some((event_id, index)) = maka_runtime::tool_call::parse_provider_result_id(message_id) {
        let used: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE event_id = ?1
                 AND kind = 'model_completed'
                 AND json_extract(event_json, '$.invocation.session_id') = ?2
                 AND json_extract(event_json, ?3) = 'tool_result')",
        )
        .bind(event_id)
        .bind(session)
        .bind(format!("$.fact.output.parts[{index}].kind"))
        .fetch_one(&mut *tx)
        .await?;
        if used {
            return Err(conflict());
        }
    }
    // Only this exact generated spelling can equal a tool ID. Ordinary
    // client UUIDs never scan tool facts; this is not a reserved prefix.
    if message_id.starts_with("tool_") {
        let used: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM runtime_events WHERE kind IN ('tool_dispatched', 'tool_rejected')
                    AND json_extract(event_json, '$.invocation.session_id') = ?1
                    AND maka_tool_use_id(invocation_id, operation_id) = ?2
                    UNION ALL
                    SELECT 1 FROM runtime_events event, json_each(event_json, '$.fact.output.parts') part
                    WHERE event.kind = 'model_completed'
                    AND json_extract(event.event_json, '$.invocation.session_id') = ?1
                    AND json_extract(part.value, '$.kind') = 'tool_call'
                    AND maka_tool_use_id(event.invocation_id, event.operation_id || ':' ||
                        json_extract(part.value, '$.call.id')) = ?2)"
            ).bind(session).bind(message_id).fetch_one(&mut *tx).await?;
        if used {
            return Err(conflict());
        }
    }
    Ok(())
}
async fn reject_claim(
    tx: &mut SqliteConnection,
    session: &str,
    id: &str,
) -> Result<(), StoreError> {
    let claimed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM message_sources WHERE session_id = ?1 AND message_id = ?2
         UNION ALL SELECT 1 FROM message_admissions WHERE session_id = ?1 AND message_id = ?2)",
    )
    .bind(session)
    .bind(id)
    .fetch_one(tx)
    .await?;
    if claimed { Err(conflict()) } else { Ok(()) }
}
fn conflict() -> StoreError {
    StoreError::InvalidTransition(
        "message identity collides with a canonical visible identity".into(),
    )
}
pub(crate) fn register(connection: &rusqlite::Connection) -> Result<(), StoreError> {
    use rusqlite::functions::FunctionFlags;
    connection.create_scalar_function(
        "maka_tool_use_id",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            Ok(tool_use_id(
                ctx.get_raw(0).as_str()?,
                ctx.get_raw(1).as_str()?,
            ))
        },
    )?;
    Ok(())
}
