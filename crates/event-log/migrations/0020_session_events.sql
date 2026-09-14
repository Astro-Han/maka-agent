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

-- One ordered ledger, with owner-specific read views. Existing execution bytes
-- and positions remain unchanged; Session facts never enter a Run prefix.
CREATE TABLE event_log (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK(sequence > 0),
    event_id TEXT NOT NULL UNIQUE,
    invocation_id TEXT,
    kind TEXT NOT NULL,
    operation_id TEXT,
    event_json TEXT NOT NULL,
    CHECK(invocation_id IS NOT NULL OR operation_id IS NULL)
);
INSERT INTO event_log SELECT * FROM runtime_events ORDER BY sequence;
UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE(
    (SELECT seq FROM sqlite_sequence WHERE name = 'runtime_events'), 0
)) WHERE name = 'event_log';

CREATE TABLE tool_result_payloads_next (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES event_log(event_id),
    payload BLOB NOT NULL CHECK(length(payload) <= 67108864)
);
INSERT INTO tool_result_payloads_next SELECT * FROM tool_result_payloads;
DROP TABLE tool_result_payloads;
DROP TABLE runtime_events;
ALTER TABLE tool_result_payloads_next RENAME TO tool_result_payloads;

CREATE VIEW runtime_events AS
    SELECT * FROM event_log WHERE invocation_id IS NOT NULL;
CREATE VIEW session_events AS
    SELECT sequence, event_id, kind, event_json FROM event_log WHERE invocation_id IS NULL;

CREATE INDEX invocation_sequence ON event_log(invocation_id, sequence);
CREATE INDEX session_event_sequence ON event_log(
    json_extract(event_json, '$.invocation.session_id'), sequence
);
CREATE INDEX session_control_sequence ON event_log(
    json_extract(event_json, '$.session_id'), sequence
) WHERE invocation_id IS NULL;
CREATE INDEX turn_opening_lookup ON event_log(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.invocation.turn_id'), sequence DESC
) WHERE kind = 'invocation_opened';
CREATE UNIQUE INDEX operation_fact ON event_log(operation_id, kind)
    WHERE operation_id IS NOT NULL;
CREATE UNIQUE INDEX invocation_boundary ON event_log(invocation_id, kind)
    WHERE kind IN ('invocation_opened', 'invocation_ended');
CREATE UNIQUE INDEX message_steering_identity ON event_log(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.fact.message.message_id')
) WHERE kind = 'message_steered';
CREATE UNIQUE INDEX continuation_claim_id ON event_log(
    json_extract(event_json, '$.fact.input.claim.id')
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') = 'continuation';
CREATE UNIQUE INDEX continuation_source_boundary ON event_log(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.fact.input.claim.source.invocation.run_id'),
    json_extract(event_json, '$.fact.input.claim.source.high_water')
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') = 'continuation';
CREATE UNIQUE INDEX workhub_action_identity ON event_log(
    CASE
        WHEN kind = 'workhub_stop_requested'
            THEN json_extract(event_json, '$.fact.intent.request.action_id')
        WHEN kind = 'workhub_delegated'
            THEN json_extract(event_json, '$.fact.delegation.action_id')
        WHEN kind = 'workhub_resume_observed'
            THEN json_extract(event_json, '$.fact.resume.action_id')
        WHEN kind = 'invocation_opened'
            AND json_extract(event_json, '$.fact.input.kind') = 'continuation'
            THEN json_extract(event_json, '$.fact.input.workhub_resume.action_id')
    END
) WHERE kind IN ('workhub_delegated', 'workhub_resume_observed', 'invocation_opened', 'workhub_stop_requested');

CREATE UNIQUE INDEX workhub_stop_resolution ON event_log(
    json_extract(event_json, '$.fact.action_id')
) WHERE kind = 'workhub_stop_resolved';
CREATE INDEX workhub_stop_request_subject ON event_log(
    json_extract(event_json, '$.fact.intent.delegation_action_id')
) WHERE kind = 'workhub_stop_requested';

-- Pre-cutover receipts have no historical sequence or timestamp. Keep them
-- readable, never invent a timeline for them, and never write new receipts here.
ALTER TABLE workhub_stops RENAME TO legacy_workhub_stops;
CREATE VIEW workhub_stops AS
    SELECT json_extract(q.event_json, '$.fact.intent.request.action_id') AS action_id,
           json_extract(q.event_json, '$.fact.intent.delegation_action_id') AS delegation_action_id,
           json_extract(q.event_json, '$.fact.intent') AS record_json,
           json_extract(r.event_json, '$.fact.resolution') AS resolution_json
    FROM session_events q LEFT JOIN session_events r
      ON r.kind = 'workhub_stop_resolved'
     AND json_extract(r.event_json, '$.fact.action_id') =
         json_extract(q.event_json, '$.fact.intent.request.action_id')
    WHERE q.kind = 'workhub_stop_requested'
    UNION ALL
    SELECT q.action_id, q.delegation_action_id, q.record_json,
           COALESCE(json_extract(r.event_json, '$.fact.resolution'), q.resolution_json)
    FROM legacy_workhub_stops q LEFT JOIN session_events r
      ON r.kind = 'workhub_stop_resolved'
     AND json_extract(r.event_json, '$.fact.action_id') = q.action_id;

PRAGMA user_version = 20;
