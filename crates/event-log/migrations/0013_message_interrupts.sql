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

PRAGMA user_version = 13;

-- A NULL fence records a rejected exact command; accepted fences revoke pending
-- delivery atomically. Terminal results remain canonical invocation facts.
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
