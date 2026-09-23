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

-- Foreign canonical evidence is immutable history, not this Host's accepted work.
-- Import publishes these origins in the same transaction as their events.
CREATE TABLE imported_invocations (
    invocation_id TEXT PRIMARY KEY,
    bundle_digest TEXT NOT NULL CHECK(length(bundle_digest) = 71)
);

-- Execution ownership queries use this view; history and proof readers retain
-- runtime_events, including unchanged foreign evidence.
CREATE VIEW local_runtime_events AS
    SELECT e.* FROM runtime_events e
    WHERE NOT EXISTS(SELECT 1 FROM imported_invocations i WHERE i.invocation_id=e.invocation_id);
