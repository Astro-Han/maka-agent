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

PRAGMA user_version = 2;

-- Read acknowledgement is durable control state, never a rebuilt event cache.
CREATE TABLE IF NOT EXISTS session_read_state (
    session_id TEXT PRIMARY KEY REFERENCES session_control(id),
    has_unread INTEGER NOT NULL CHECK(has_unread IN (0, 1)),
    last_read_message_id TEXT
);

-- Existing Rust sessions predate acknowledgement support. Terminal history
-- initializes unread once; subsequent cache reconstruction cannot change it.
INSERT OR IGNORE INTO session_read_state (session_id, has_unread, last_read_message_id)
SELECT id, EXISTS(
    SELECT 1 FROM runtime_events
    WHERE json_extract(event_json, '$.invocation.session_id') = session_control.id
      AND kind = 'invocation_ended'
), NULL FROM session_control;
