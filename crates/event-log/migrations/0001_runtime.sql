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

-- Canonical facts and durable control state. Disposable projections are built at startup.

CREATE TABLE event_log (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK(sequence > 0),
    event_id TEXT NOT NULL UNIQUE,
    invocation_id TEXT,
    kind TEXT NOT NULL,
    operation_id TEXT,
    event_json TEXT NOT NULL,
    CHECK(invocation_id IS NOT NULL OR operation_id IS NULL)
);

CREATE TABLE session_control (
    id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived INTEGER NOT NULL CHECK(archived IN (0, 1)),
    configuration TEXT NOT NULL
);

CREATE TABLE session_catalog_revision (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision >= 0)
);

CREATE TABLE session_read_state (
    session_id TEXT PRIMARY KEY REFERENCES session_control(id),
    has_unread INTEGER NOT NULL CHECK(has_unread IN (0, 1)),
    last_read_message_id TEXT
);

CREATE TABLE interaction_requests (
    request_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 20480)
);

CREATE INDEX interaction_session_pending ON interaction_requests(session_id, created_at, request_id);

CREATE INDEX interaction_permission_basis ON interaction_requests(
    session_id, json_extract(record_json, '$.request.baseRevision'), created_at, request_id
) WHERE json_extract(record_json, '$.request.kind') = 'permissions';

CREATE TABLE interaction_outcomes (
    request_id TEXT PRIMARY KEY REFERENCES interaction_requests(request_id),
    outcome_json TEXT NOT NULL CHECK(length(CAST(outcome_json AS BLOB)) <= 8192)
);

CREATE TABLE client_capability_session_grants (
    authority_key TEXT PRIMARY KEY,
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 12288)
);

CREATE TABLE artifacts (
    session_id TEXT NOT NULL REFERENCES session_control(id),
    id TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 16384),
    content_sha256 TEXT NOT NULL,
    payload BLOB NOT NULL CHECK(length(payload) <= 52428800),
    PRIMARY KEY (session_id, id),
    CHECK(json_extract(record_json, '$.sessionId') = session_id),
    CHECK(json_extract(record_json, '$.id') = id),
    CHECK(json_extract(record_json, '$.createdAt') = created_at),
    CHECK(json_extract(record_json, '$.sizeBytes') = length(payload))
);

CREATE INDEX artifacts_session_order ON artifacts(session_id, created_at DESC, id);

CREATE TABLE artifact_catalog (
    session_id TEXT PRIMARY KEY REFERENCES session_control(id),
    revision INTEGER NOT NULL CHECK(revision > 0 AND revision <= 9007199254740991)
);

CREATE TABLE shell_runs (
    session_id TEXT NOT NULL REFERENCES session_control(id),
    id TEXT NOT NULL,
    started_at INTEGER NOT NULL CHECK(started_at >= 0),
    active INTEGER NOT NULL CHECK(active IN (0, 1)),
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 4194304),
    PRIMARY KEY (session_id, id),
    CHECK(json_extract(record_json, '$.sessionId') IS session_id),
    CHECK(json_extract(record_json, '$.id') IS id),
    CHECK(json_extract(record_json, '$.startedAt') IS started_at),
    CHECK(active IS (json_extract(record_json, '$.state.kind') IN ('starting', 'running')))
);

CREATE INDEX shell_runs_session_order ON shell_runs(session_id, started_at, id);

CREATE INDEX shell_runs_active ON shell_runs(active) WHERE active = 1;

CREATE TABLE message_admissions (
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(session_id, message_id)
);

CREATE INDEX message_admission_order ON message_admissions(session_id, position, message_id);

CREATE TABLE message_queue_state (
    session_id TEXT PRIMARY KEY NOT NULL,
    revision INTEGER NOT NULL CHECK(revision >= 0)
);

CREATE TABLE message_cancellations (
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    cancellation_id TEXT NOT NULL,
    PRIMARY KEY(session_id, message_id)
);

CREATE TABLE queue_command_receipts (
    host_epoch TEXT NOT NULL,
    session_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    command_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY(host_epoch, session_id, operation, command_id)
);

CREATE TABLE message_submit_receipts (
    host_epoch TEXT NOT NULL,
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY(host_epoch, session_id, message_id)
);

CREATE TABLE message_interrupt_receipts (
    host_epoch TEXT NOT NULL,
    session_id TEXT NOT NULL,
    interrupt_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    fence_json TEXT,
    PRIMARY KEY(host_epoch, session_id, interrupt_id)
);

CREATE INDEX message_interrupt_run ON message_interrupt_receipts(session_id, run_id)
    WHERE fence_json IS NOT NULL;

CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL,
    identity TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 16384),
    last_used_at INTEGER NOT NULL CHECK(last_used_at BETWEEN 0 AND 9007199254740991),
    archived_at INTEGER CHECK(archived_at BETWEEN 0 AND 9007199254740991)
) STRICT;

CREATE TABLE project_identities (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX project_identity_owner ON project_identities(project_id, id);

CREATE TABLE project_locations (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL CHECK(length(CAST(path AS BLOB)) BETWEEN 1 AND 4096),
    is_worktree INTEGER NOT NULL CHECK(is_worktree IN (0, 1)),
    last_used_at INTEGER NOT NULL CHECK(last_used_at BETWEEN 0 AND 9007199254740991),
    PRIMARY KEY(project_id, path)
) STRICT;

CREATE UNIQUE INDEX pending_root_invocation ON message_admissions (
    json_extract(record_json, '$.invocation.invocation_id')
) WHERE json_extract(record_json, '$.source.disposition') = 'turn_started';

CREATE TABLE "tool_result_payloads" (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES event_log(event_id),
    payload BLOB NOT NULL CHECK(length(payload) <= 67108864)
);

CREATE VIEW runtime_events AS
    SELECT * FROM event_log WHERE invocation_id IS NOT NULL;

CREATE INDEX invocation_sequence ON event_log(invocation_id, sequence);

CREATE INDEX session_event_sequence ON event_log(
    json_extract(event_json, '$.invocation.session_id'), sequence
);

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
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') IN ('continuation','handoff');

CREATE UNIQUE INDEX continuation_source_boundary ON event_log(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.fact.input.claim.source.invocation.run_id'),
    json_extract(event_json, '$.fact.input.claim.source.high_water')
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') IN ('continuation','handoff');

CREATE UNIQUE INDEX handoff_successor_run ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.successor_run_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';

CREATE UNIQUE INDEX handoff_successor_invocation ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.successor_invocation_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';

CREATE UNIQUE INDEX handoff_claim ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.claim_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';

CREATE TABLE plugin_composition (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    ledger_json TEXT NOT NULL CHECK (json_valid(ledger_json))
);

CREATE TABLE plugin_package_blobs (
    digest TEXT PRIMARY KEY
);

CREATE TABLE plugin_package_files (
    digest TEXT NOT NULL REFERENCES plugin_package_blobs(digest) ON DELETE CASCADE,
    path TEXT NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY (digest, path)
);

CREATE TABLE plugin_packages (
    id TEXT PRIMARY KEY,
    digest TEXT NOT NULL REFERENCES plugin_package_blobs(digest)
);

CREATE TABLE plugin_data (
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    key TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    value_json TEXT CHECK (value_json IS NULL OR json_valid(value_json)),
    PRIMARY KEY (package_id, scope_id, key)
);

CREATE TABLE plugin_message_receipts (
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY (package_id, scope_id, operation_id)
);

CREATE TABLE plugin_execution_receipts (
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    PRIMARY KEY (package_id, scope_id, operation_id)
);

CREATE TABLE request_compositions (
    digest TEXT PRIMARY KEY NOT NULL,
    surface BLOB NOT NULL CHECK(length(surface) <= 4194304)
);

CREATE TABLE model_request_compositions (
    event_id TEXT PRIMARY KEY NOT NULL REFERENCES event_log(event_id),
    digest TEXT NOT NULL REFERENCES request_compositions(digest)
);

CREATE INDEX model_request_composition_digest ON model_request_compositions(digest);

CREATE TABLE session_managers (
    session_id TEXT PRIMARY KEY NOT NULL,
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL
);

CREATE TABLE host_effects (
    id TEXT PRIMARY KEY,
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    request TEXT NOT NULL CHECK(json_valid(request) AND length(CAST(request AS BLOB)) <= 8388608),
    outcome TEXT CHECK(outcome IS NULL OR (json_valid(outcome) AND length(CAST(outcome AS BLOB)) <= 8388608)),
    payload BLOB CHECK(payload IS NULL OR length(payload) <= 5242880),
    CHECK(outcome IS NOT NULL OR payload IS NULL)
) STRICT;

CREATE INDEX host_effects_namespace ON host_effects(package_id, scope_id);

-- Query-independent text projection; canonical events remain authoritative.
CREATE TABLE transcript_text (
    sequence INTEGER PRIMARY KEY,
    timestamp INTEGER NOT NULL,
    role TEXT NOT NULL,
    body BLOB NOT NULL
);

CREATE TABLE message_sources (
    session_id TEXT NOT NULL, message_id TEXT NOT NULL, event_id TEXT NOT NULL,
    PRIMARY KEY(session_id, message_id)
);

CREATE TABLE transcript_rows (
    sequence INTEGER PRIMARY KEY CHECK(sequence >= 0),
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    digest TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes > 0),
    UNIQUE(session_id, message_id)
);

CREATE INDEX transcript_session_sequence
    ON transcript_rows(session_id, sequence);

CREATE INDEX transcript_session_turn
    ON transcript_rows(session_id, turn_id, sequence);

CREATE TABLE transcript_progress (
    session_id TEXT PRIMARY KEY,
    through_sequence INTEGER NOT NULL CHECK(through_sequence >= 0)
);

CREATE INDEX catalog_message_facts ON event_log(
    json_extract(event_json, '$.invocation.session_id'), kind, sequence
) WHERE kind IN ('invocation_opened', 'model_completed');

CREATE INDEX catalog_part_starts ON event_log(
    json_extract(event_json, '$.invocation.session_id'), invocation_id,
    json_extract(event_json, '$.fact.step_id'), sequence
) WHERE kind = 'model_observed'
    AND json_extract(event_json, '$.fact.event.kind') = 'part_started';

CREATE TABLE catalog_messages (
    sequence INTEGER NOT NULL, ordinal INTEGER NOT NULL, session_id TEXT NOT NULL,
    message_at INTEGER NOT NULL, preview TEXT, message_id TEXT NOT NULL,
    PRIMARY KEY (sequence, ordinal)
);

CREATE INDEX catalog_visible_tail ON catalog_messages(
    session_id, sequence DESC, ordinal DESC
);

CREATE INDEX catalog_latest_message ON catalog_messages(
    session_id, message_at DESC, sequence DESC, ordinal DESC
);

CREATE INDEX catalog_latest_preview ON catalog_messages(
    session_id, message_at DESC, sequence DESC, ordinal DESC
) WHERE preview IS NOT NULL;

CREATE TABLE catalog_message_watermark(
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1), sequence INTEGER NOT NULL
);

CREATE INDEX catalog_partial_deltas ON event_log(
    invocation_id, json_extract(event_json, '$.fact.step_id'),
    json_extract(event_json, '$.fact.event.data.id'), sequence
) WHERE kind = 'model_observed'
    AND json_extract(event_json, '$.fact.event.kind') = 'part_delta';

CREATE INDEX catalog_model_boundaries ON event_log(
    invocation_id, kind, sequence
) WHERE kind IN ('model_requested', 'model_completed', 'model_interrupted');

CREATE INDEX navigation_boundaries ON event_log(
    json_extract(event_json, '$.invocation.session_id'), kind, sequence
) WHERE kind IN ('invocation_opened', 'invocation_ended');

INSERT INTO session_catalog_revision VALUES (1, 0);
INSERT INTO plugin_composition VALUES (1, '{"generation":0,"packageLayers":[],"overlays":[]}');

PRAGMA user_version = 1;
