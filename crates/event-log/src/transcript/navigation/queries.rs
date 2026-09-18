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

pub(super) const FENCE: &str = "
SELECT MAX(event.sequence) FROM runtime_events event
JOIN runtime_events opening ON opening.invocation_id = event.invocation_id AND opening.kind = 'invocation_opened'
WHERE json_extract(event.event_json, '$.invocation.session_id') = ?1
AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation', 'handoff')";

pub(super) const ROWS: &str = "
SELECT row.sequence, row.turn_id,
 CASE WHEN source.kind IN ('invocation_opened', 'message_steered') THEN
   navigation_preview(COALESCE(json_extract(row.payload, '$.displayText'),
                              json_extract(row.payload, '$.text')), 256) END,
 CASE WHEN source.kind = 'invocation_ended'
      AND json_extract(row.payload, '$.type') = 'turn_state' THEN row.payload END
FROM transcript_rows row
JOIN runtime_events source ON source.sequence = row.sequence / 256
WHERE row.session_id = ?1 AND row.sequence <= ?2 AND row.sequence >= ?3
ORDER BY row.sequence LIMIT 257";

pub(super) const LANDMARKS: &str = "
WITH openings AS (
 SELECT opening.invocation_id, opening.sequence,
 ROW_NUMBER() OVER (PARTITION BY json_extract(opening.event_json, '$.invocation.turn_id') ORDER BY opening.sequence) AS first
 FROM runtime_events opening
 WHERE json_extract(opening.event_json, '$.invocation.session_id') = ?1
 AND opening.kind = 'invocation_opened' AND opening.sequence <= ?2
 AND json_extract(opening.event_json, '$.fact.input.kind') IN ('message', 'continuation')
), candidates AS (
 SELECT invocation_id, sequence, ROW_NUMBER() OVER (ORDER BY sequence) - 1 AS rank, COUNT(*) OVER () AS total
 FROM openings WHERE first = 1
), samples(n) AS (
 SELECT 0 UNION ALL SELECT n + 1 FROM samples WHERE n + 1 < ?3
)
SELECT DISTINCT invocation_id, sequence FROM candidates
JOIN samples ON rank = CASE WHEN ?3 = 1 THEN total - 1 ELSE n * (total - 1) / (?3 - 1) END
ORDER BY sequence";

pub(super) const PROMPT: &str = "
SELECT row.sequence, row.turn_id,
 navigation_preview(COALESCE(json_extract(row.payload, '$.displayText'),
                             json_extract(row.payload, '$.text')), 96)
FROM runtime_events source
JOIN transcript_rows row ON row.sequence >= source.sequence * 256 AND row.sequence < (source.sequence + 1) * 256
WHERE source.invocation_id = ?2 AND source.kind IN ('invocation_opened', 'message_steered')
AND row.session_id = ?1 AND row.sequence <= ?3
ORDER BY row.sequence LIMIT 1";
