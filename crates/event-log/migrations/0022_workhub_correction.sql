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

-- Assignments keep their original owner and bytes. Replacement commits have
-- Session ownership; the intent, not the later assignment, owns the action ID.
DROP INDEX workhub_action_identity;
CREATE UNIQUE INDEX workhub_action_identity ON event_log(
    CASE
        WHEN kind IN ('workhub_stop_requested', 'workhub_correction_requested')
            THEN json_extract(event_json, '$.fact.intent.request.action_id')
        WHEN kind = 'workhub_delegated' AND invocation_id IS NOT NULL
            THEN json_extract(event_json, '$.fact.delegation.action_id')
        WHEN kind = 'workhub_resume_observed'
            THEN json_extract(event_json, '$.fact.resume.action_id')
        WHEN kind = 'invocation_opened'
            AND json_extract(event_json, '$.fact.input.kind') = 'continuation'
            THEN json_extract(event_json, '$.fact.input.workhub_resume.action_id')
    END
) WHERE kind IN ('workhub_delegated', 'workhub_resume_observed', 'invocation_opened', 'workhub_stop_requested', 'workhub_correction_requested');

CREATE UNIQUE INDEX workhub_assignment_action ON event_log(
    json_extract(event_json, '$.fact.delegation.action_id')
) WHERE kind = 'workhub_delegated';
CREATE UNIQUE INDEX workhub_correction_subject ON event_log(
    json_extract(event_json, '$.fact.intent.request.replaces_action_id')
) WHERE kind = 'workhub_correction_requested';
CREATE UNIQUE INDEX workhub_correction_resolution ON event_log(
    json_extract(event_json, '$.fact.action_id')
) WHERE kind IN ('workhub_superseded', 'workhub_correction_aborted');

CREATE VIEW workhub_assignments AS
    SELECT sequence, event_id, event_json,
           COALESCE(json_extract(event_json, '$.invocation'),
                    json_extract(event_json, '$.fact.coordinator')) AS coordinator_json
    FROM event_log WHERE kind = 'workhub_delegated';

CREATE VIEW workhub_corrections AS
    SELECT q.sequence,
           json_extract(q.event_json, '$.fact.intent.request.action_id') AS action_id,
           json_extract(q.event_json, '$.fact.intent.request.replaces_action_id') AS replaces_action_id,
           json_extract(q.event_json, '$.fact.intent') AS intent_json,
           r.kind AS resolution_kind,
           json_extract(r.event_json, '$.fact.reason') AS abort_reason
    FROM session_events q LEFT JOIN session_events r
      ON r.kind IN ('workhub_superseded', 'workhub_correction_aborted')
     AND json_extract(r.event_json, '$.fact.action_id') =
         json_extract(q.event_json, '$.fact.intent.request.action_id')
    WHERE q.kind = 'workhub_correction_requested';

PRAGMA user_version = 22;

