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


-- Transfer receipts survive Session deletion; replaying a committed import never
-- recreates its catalog. Membership records preserve topology, not plugin grants.
ALTER TABLE session_history_copies ADD COLUMN bundle_digest TEXT
    CHECK(bundle_digest IS NULL OR length(bundle_digest)=71);

CREATE TABLE session_bundle_imports (
    digest TEXT PRIMARY KEY CHECK(length(digest)=71),
    binding_digest TEXT NOT NULL CHECK(length(binding_digest)=71),
    receipt_json TEXT NOT NULL CHECK(json_valid(receipt_json))
);
CREATE TABLE session_bundle_members (
    session_id TEXT PRIMARY KEY,
    bundle_digest TEXT NOT NULL REFERENCES session_bundle_imports(digest),
    parent_session_id TEXT,
    CHECK(parent_session_id IS NULL OR parent_session_id!=session_id)
);
CREATE INDEX bundle_parent_sessions ON session_bundle_members(parent_session_id, session_id);
