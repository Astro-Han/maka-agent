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

use crate::{EventLog, StoreError, message_resolution::MessageExecution};
use sqlx::Connection;

pub struct AssistantExcerpt {
    pub text: String,
    pub complete: bool,
}

/// Exact delivery lineage and its terminal answer from one read snapshot.
pub struct MessageObservation {
    pub execution: MessageExecution,
    pub answer: Option<AssistantExcerpt>,
}

impl EventLog {
    pub async fn message_observation(
        &self,
        session: &str,
        message: &str,
    ) -> Result<MessageObservation, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        crate::sessions::validate_id(message)?;
        let (session, message) = (session.to_owned(), message.to_owned());
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            let execution = crate::message_resolution::owner::execution(&mut tx, &session, &message).await?;
            let answer = match &execution {
                MessageExecution::Owned(boundary) | MessageExecution::Shared(boundary)
                    if boundary.state.terminal_outcome().is_some() => {
                    // Do not hydrate model/tool bodies or scan another Run's output.
                    // The extra scalar distinguishes a complete excerpt from a cut.
                    let text: Option<String> = sqlx::query_scalar(
                        "SELECT substr(text, 1, 16385) FROM (
                            SELECT sequence, json_extract(event_json, '$.fact.text') AS text
                            FROM runtime_events WHERE invocation_id = ?1 AND kind = 'executor_completed'
                            UNION ALL
                            SELECT event.sequence, (
                                SELECT group_concat(json_extract(part.value, '$.text'), '')
                                FROM json_each(event.event_json, '$.fact.output.parts') part
                                WHERE json_extract(part.value, '$.kind') = 'text'
                                  AND json_extract(part.value, '$.text_kind') = 'text'
                            ) AS text
                            FROM runtime_events event WHERE invocation_id = ?1 AND kind = 'model_completed'
                        ) WHERE text IS NOT NULL AND trim(text) != ''
                        ORDER BY sequence DESC LIMIT 1"
                    ).bind(&boundary.invocation.invocation_id).fetch_optional(&mut *tx).await?;
                    text.map(|text| {
                        let complete = text.chars().count() <= 16384;
                        AssistantExcerpt { text: text.chars().take(16384).collect(), complete }
                    })
                }
                _ => None,
            };
            Ok(MessageObservation { execution, answer })
        })).await
    }
}
