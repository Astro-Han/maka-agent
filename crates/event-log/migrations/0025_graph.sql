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

CREATE TABLE graph_epochs (
    root_session_id TEXT NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch > 0),
    graph_id TEXT NOT NULL UNIQUE,
    mode TEXT NOT NULL CHECK(mode IN ('graph', 'swarm')),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    stop_requested INTEGER NOT NULL DEFAULT 0 CHECK(stop_requested IN (0, 1)),
    PRIMARY KEY(root_session_id, epoch)
);
CREATE TABLE graph_updates (
    graph_id TEXT NOT NULL REFERENCES graph_epochs(graph_id),
    update_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    fingerprint TEXT NOT NULL,
    update_json TEXT NOT NULL CHECK(json_valid(update_json)),
    byte_count INTEGER GENERATED ALWAYS AS (length(CAST(update_json AS BLOB))) STORED,
    committed_at INTEGER NOT NULL CHECK(committed_at >= 0),
    PRIMARY KEY(graph_id, revision),
    UNIQUE(graph_id, update_id)
);
CREATE TABLE graph_intents (
    graph_id TEXT NOT NULL REFERENCES graph_epochs(graph_id),
    work_id TEXT NOT NULL,
    intent_json TEXT NOT NULL CHECK(json_valid(intent_json)),
    PRIMARY KEY(graph_id, work_id)
);
CREATE TABLE graph_wakes (
    graph_id TEXT NOT NULL REFERENCES graph_epochs(graph_id),
    snapshot_key TEXT NOT NULL,
    request_json TEXT NOT NULL CHECK(json_valid(request_json)),
    PRIMARY KEY(graph_id, snapshot_key)
);
PRAGMA user_version = 25;
