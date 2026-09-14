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


-- Artifact control authority is independent of invocation history. Metadata and
-- immutable payload become visible together; query projections never scan BLOBs.
PRAGMA user_version = 4;
CREATE TABLE IF NOT EXISTS artifacts (
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
CREATE INDEX IF NOT EXISTS artifacts_session_order ON artifacts(session_id, created_at DESC, id);
CREATE TABLE IF NOT EXISTS artifact_catalog (
    session_id TEXT PRIMARY KEY REFERENCES session_control(id),
    revision INTEGER NOT NULL CHECK(revision > 0 AND revision <= 9007199254740991)
);
