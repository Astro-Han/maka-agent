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
    -- Keep compact accounting fields before potentially large JSON overflow
    -- pages. SQLite derives them from canonical bytes; they cannot be edited.
    accounting_at REAL GENERATED ALWAYS AS (
        CASE WHEN kind IN ('model_requested', 'model_completed', 'model_interrupted',
                           'invocation_ended', 'auxiliary_model_started', 'auxiliary_model_settled')
        THEN COALESCE(json_extract(event_json, '$.recorded_at.secs_since_epoch'),
                      json_extract(event_json, '$.started_at.secs_since_epoch'),
                      json_extract(event_json, '$.completed_at.secs_since_epoch')) * 1000.0
            + COALESCE(json_extract(event_json, '$.recorded_at.nanos_since_epoch'),
                       json_extract(event_json, '$.started_at.nanos_since_epoch'),
                       json_extract(event_json, '$.completed_at.nanos_since_epoch')) / 1000000.0
        END
    ) STORED,
    accounting_usage TEXT GENERATED ALWAYS AS (
        CASE
            WHEN kind = 'model_completed' THEN json_extract(event_json, '$.fact.output.usage')
            WHEN kind = 'model_observed' AND json_extract(event_json, '$.fact.event.kind') = 'finished'
                THEN json_extract(event_json, '$.fact.event.data.usage')
            WHEN kind = 'auxiliary_model_usage' THEN json_extract(event_json, '$.usage')
        END
    ) STORED,
    invocation_model TEXT GENERATED ALWAYS AS (
        CASE WHEN kind = 'invocation_opened' THEN json_extract(event_json, '$.fact.configuration.model') END
    ) STORED,
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

-- Retirement fences new admissions before asynchronous resource cleanup. Keep
-- its identity after cleanup so a lost reply cannot resurrect the Session.
CREATE TABLE session_retirements (
    session_id TEXT PRIMARY KEY,
    remove_session INTEGER NOT NULL DEFAULT 1 CHECK(remove_session IN (0, 1)),
    completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0, 1))
);

CREATE TABLE session_removal_receipts (
    session_id TEXT PRIMARY KEY,
    plan_json TEXT NOT NULL
);

-- A Host crash loses native handles, not proof that a process may still write.
CREATE TABLE session_processes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    cleaned INTEGER NOT NULL DEFAULT 0 CHECK(cleaned IN (0, 1))
);
CREATE INDEX session_processes_pending ON session_processes(session_id) WHERE cleaned=0;

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

-- Copies retain original facts, never execution authority. Flatten membership
-- at creation so descendants do not query mutable parent Session metadata.
CREATE TABLE session_history_copies (
    session_id TEXT PRIMARY KEY,
    source_session_id TEXT NOT NULL,
    source_revision INTEGER NOT NULL CHECK(source_revision > 0),
    through_sequence INTEGER NOT NULL CHECK(through_sequence >= 0),
    observed_through INTEGER NOT NULL CHECK(observed_through >= through_sequence),
    request_json TEXT NOT NULL,
    lineage_json TEXT NOT NULL CHECK(json_valid(lineage_json)),
    state TEXT NOT NULL CHECK(state IN ('preparing', 'committed', 'abandoned')),
    CHECK(state = 'committed' OR json_extract(lineage_json, '$.kind') = 'revision'),
    CHECK(session_id != source_session_id)
);

CREATE UNIQUE INDEX session_revision_family ON session_history_copies(
    CAST(json_extract(lineage_json, '$.root_session_id') AS TEXT),
    CAST(json_extract(lineage_json, '$.index') AS INTEGER)
) WHERE json_extract(lineage_json, '$.kind') = 'revision';

CREATE TABLE session_history_members (
    session_id TEXT NOT NULL REFERENCES session_history_copies(session_id),
    sequence INTEGER NOT NULL REFERENCES event_log(sequence),
    archive_sequence INTEGER REFERENCES event_log(sequence),
    PRIMARY KEY(session_id, sequence)
);

-- Revision input is editable evidence, not part of the destination conversation.
CREATE TABLE session_revision_sources (
    session_id TEXT NOT NULL REFERENCES session_history_copies(session_id),
    sequence INTEGER NOT NULL REFERENCES event_log(sequence),
    PRIMARY KEY(session_id, sequence)
);

CREATE VIEW session_history_events AS
    SELECT json_extract(event_json, '$.invocation.session_id') AS owner_session_id,
           0 AS inherited, NULL AS archive_sequence, e.* FROM runtime_events e
    UNION ALL
    SELECT h.session_id, 1, h.archive_sequence, e.*
    FROM session_history_members h JOIN runtime_events e ON e.sequence = h.sequence;

CREATE TABLE session_history_artifacts (
    session_id TEXT NOT NULL REFERENCES session_history_copies(session_id),
    source_session_id TEXT NOT NULL,
    source_artifact_id TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    PRIMARY KEY(session_id, source_session_id, source_artifact_id),
    FOREIGN KEY(session_id, artifact_id) REFERENCES artifacts(session_id, id)
);

CREATE INDEX invocation_sequence ON event_log(invocation_id, sequence);

CREATE UNIQUE INDEX archived_tool_result ON event_log(
    CAST(json_extract(event_json, '$.fact.placeholder.identity.runtime_event_id') AS TEXT),
    json_extract(event_json, '$.invocation.session_id')
) WHERE kind = 'tool_result_archived';

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

CREATE INDEX model_usage_requests ON event_log(sequence) WHERE kind = 'model_requested';

-- Quote admission and provider-usage valuation share their source fact's commit.
-- These immutable records are canonical accounting, not recalculated projections.
CREATE TABLE model_accounting (
    request_id TEXT PRIMARY KEY REFERENCES event_log(event_id),
    quote_json TEXT NOT NULL CHECK(json_valid(quote_json) AND length(CAST(quote_json AS BLOB)) <= 8192)
);
CREATE TABLE model_valuations (
    request_id TEXT PRIMARY KEY REFERENCES model_accounting(request_id),
    usage_json TEXT NOT NULL CHECK(json_valid(usage_json)),
    usd REAL CHECK(usd IS NULL OR usd >= 0)
);

CREATE INDEX model_usage_observation ON event_log(
    invocation_id, json_extract(event_json, '$.fact.step_id'), sequence DESC
) WHERE kind = 'model_observed' AND json_extract(event_json, '$.fact.event.kind') = 'finished';

-- Count physical admissions, not history memberships or accepted responses.
-- A rejected response can still have provider-reported usage. No mutable
-- Session metadata or current provider configuration participates in this view.
CREATE VIEW agent_model_usage AS
WITH attempts AS (
    SELECT request.sequence, request.event_id,
        json_extract(request.event_json, '$.invocation') AS invocation,
        opening.invocation_model AS binding,
        json_extract(request.event_json, '$.fact.model_id') AS model_id,
        json_extract(request.event_json, '$.fact.purpose') AS purpose,
        request.accounting_at AS started_at,
        COALESCE(completed.accounting_at, interrupted.accounting_at, terminal.accounting_at) AS completed_at,
        COALESCE(completed.sequence, interrupted.sequence, terminal.sequence) AS completed_sequence,
        CASE
            WHEN completed.sequence IS NOT NULL THEN 'success'
            WHEN json_extract(interrupted.event_json, '$.fact.status') = 'cancelled' THEN 'aborted'
            WHEN interrupted.sequence IS NOT NULL THEN 'error'
            WHEN json_extract(terminal.event_json, '$.fact.outcome.kind') = 'cancelled' THEN 'aborted'
            WHEN terminal.sequence IS NOT NULL THEN 'unknown'
        END AS outcome,
        COALESCE(completed.accounting_usage, (
            SELECT observed.accounting_usage
            FROM event_log observed INDEXED BY model_usage_observation
            WHERE observed.kind = 'model_observed'
              AND json_extract(observed.event_json, '$.fact.event.kind') = 'finished'
              AND observed.invocation_id = request.invocation_id
              AND json_extract(observed.event_json, '$.fact.step_id') = request.operation_id
            ORDER BY observed.sequence DESC LIMIT 1
        ), '{}') AS usage
    FROM runtime_events request
    JOIN runtime_events opening ON opening.invocation_id = request.invocation_id
        AND opening.kind = 'invocation_opened'
    LEFT JOIN runtime_events completed ON completed.operation_id = request.operation_id
        AND completed.kind = 'model_completed'
    LEFT JOIN runtime_events interrupted ON interrupted.operation_id = request.operation_id
        AND interrupted.kind = 'model_interrupted'
    LEFT JOIN runtime_events terminal ON terminal.invocation_id = request.invocation_id
        AND terminal.kind = 'invocation_ended'
    WHERE request.kind = 'model_requested'
)
SELECT sequence, event_id,
    json_object('kind', 'agent', 'invocation', json(invocation), 'purpose', purpose) AS origin,
    binding, model_id, outcome, usage,
    json_extract(invocation, '$.session_id') AS session_id,
    started_at, completed_at,
    completed_sequence
FROM attempts;

CREATE UNIQUE INDEX auxiliary_model_source ON event_log(json_extract(event_json, '$.source'))
    WHERE kind = 'auxiliary_model_started';
CREATE UNIQUE INDEX auxiliary_model_outcome ON event_log(
    json_extract(event_json, '$.request_id'), kind
) WHERE kind IN ('auxiliary_model_settled', 'auxiliary_model_usage');

CREATE VIEW auxiliary_model_usage AS
SELECT started.sequence, started.event_id,
    json_object('kind', 'auxiliary', 'source', json_extract(started.event_json, '$.source')) AS origin,
    json_extract(started.event_json, '$.binding') AS binding,
    json_extract(started.event_json, '$.binding.model') AS model_id,
    json_extract(settled.event_json, '$.outcome') AS outcome,
    COALESCE(observed.accounting_usage, '{}') AS usage,
    json_extract(started.event_json, '$.session_id') AS session_id,
    started.accounting_at AS started_at, settled.accounting_at AS completed_at,
    settled.sequence AS completed_sequence
FROM event_log started
LEFT JOIN event_log settled ON settled.kind = 'auxiliary_model_settled'
    AND json_extract(settled.event_json, '$.request_id') = started.event_id
LEFT JOIN event_log observed ON observed.kind = 'auxiliary_model_usage'
    AND json_extract(observed.event_json, '$.request_id') = started.event_id
WHERE started.kind = 'auxiliary_model_started';

CREATE VIEW model_usage AS
SELECT calls.*, accounting.quote_json, valuation.usd
FROM (SELECT * FROM agent_model_usage UNION ALL SELECT * FROM auxiliary_model_usage) calls
LEFT JOIN model_accounting accounting ON accounting.request_id = calls.event_id
LEFT JOIN model_valuations valuation ON valuation.request_id = calls.event_id;

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

CREATE TABLE plugin_sessions (
    session_id TEXT PRIMARY KEY NOT NULL,
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    managed INTEGER NOT NULL CHECK(managed IN (0, 1)),
    authority_session_id TEXT,
    CHECK(authority_session_id IS NULL OR authority_session_id != session_id)
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
    session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    role TEXT NOT NULL,
    body BLOB NOT NULL,
    PRIMARY KEY(session_id, sequence)
);

CREATE TABLE message_sources (
    session_id TEXT NOT NULL, message_id TEXT NOT NULL, event_id TEXT NOT NULL,
    PRIMARY KEY(session_id, message_id)
);

CREATE INDEX message_source_identity ON message_sources(message_id, event_id);

CREATE INDEX message_openings_by_turn ON event_log(
    CAST(json_extract(event_json, '$.invocation.turn_id') AS TEXT), sequence
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') = 'message';

CREATE VIEW session_message_sources AS
    SELECT session_id AS owner_session_id, session_id AS source_session_id, message_id, event_id
    FROM message_sources
    UNION ALL
    SELECT h.session_id, s.session_id, s.message_id, s.event_id FROM session_history_members h
    JOIN runtime_events e ON e.sequence=h.sequence JOIN message_sources s ON s.event_id=e.event_id
    UNION ALL
    SELECT r.session_id, s.session_id, s.message_id, s.event_id FROM session_revision_sources r
    JOIN runtime_events e ON e.sequence=r.sequence JOIN message_sources s ON s.event_id=e.event_id;

CREATE TABLE transcript_rows (
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    digest TEXT NOT NULL,
    total_bytes INTEGER NOT NULL CHECK(total_bytes > 0),
    PRIMARY KEY(session_id, sequence),
    UNIQUE(session_id, message_id)
);

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
