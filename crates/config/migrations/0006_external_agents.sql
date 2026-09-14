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

-- Preserve full old documents when the new field exceeds the wire budget.
-- Normal writes still enforce 48 KiB; recovery must not prevent shrinking them.
CREATE TABLE runtime_policy_next (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    document TEXT NOT NULL CHECK(
        json_valid(document) AND length(CAST(document AS BLOB)) <= 65536
    )
);
INSERT INTO runtime_policy_next
SELECT singleton, json_insert(document, '$.policy.externalAgents',
    json('{"antigravity":{"executable":""}}')) FROM runtime_policy;
DROP TABLE runtime_policy;
ALTER TABLE runtime_policy_next RENAME TO runtime_policy;

PRAGMA user_version = 6;
