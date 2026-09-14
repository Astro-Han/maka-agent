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


-- Canonical execution facts and independent Session control state.
-- Disposable transcript/catalog projections are rebuilt from these facts.
PRAGMA application_id = 1296124754;
PRAGMA user_version = 1;

CREATE TABLE IF NOT EXISTS runtime_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT CHECK(sequence > 0),
    event_id TEXT NOT NULL UNIQUE,
    invocation_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    operation_id TEXT,
    event_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS invocation_sequence ON runtime_events(invocation_id, sequence);
CREATE INDEX IF NOT EXISTS session_event_sequence ON runtime_events(
    json_extract(event_json, '$.invocation.session_id'), sequence
);
CREATE INDEX IF NOT EXISTS turn_opening_lookup ON runtime_events(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.invocation.turn_id'), sequence DESC
) WHERE kind = 'invocation_opened';
CREATE UNIQUE INDEX IF NOT EXISTS operation_fact ON runtime_events(operation_id, kind)
    WHERE operation_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS invocation_boundary ON runtime_events(invocation_id, kind)
    WHERE kind IN ('invocation_opened', 'invocation_ended');

CREATE TABLE IF NOT EXISTS session_control (
    id TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived INTEGER NOT NULL CHECK(archived IN (0, 1)),
    configuration TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS session_catalog_revision (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision >= 0)
);
INSERT OR IGNORE INTO session_catalog_revision VALUES (1, 0);
