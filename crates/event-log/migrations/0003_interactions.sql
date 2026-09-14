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

-- Independent canonical interaction facts; grants never derive from UI history.
PRAGMA user_version = 3;
CREATE TABLE IF NOT EXISTS interaction_requests (
    request_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 20480)
);
CREATE INDEX IF NOT EXISTS interaction_session_pending ON interaction_requests(session_id, created_at, request_id);
CREATE TABLE IF NOT EXISTS interaction_outcomes (
    request_id TEXT PRIMARY KEY REFERENCES interaction_requests(request_id),
    outcome_json TEXT NOT NULL CHECK(length(CAST(outcome_json AS BLOB)) <= 8192)
);
CREATE TABLE IF NOT EXISTS client_capability_session_grants (
    authority_key TEXT PRIMARY KEY,
    record_json TEXT NOT NULL CHECK(length(CAST(record_json AS BLOB)) <= 12288)
);
