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

CREATE UNIQUE INDEX workhub_action_identity ON runtime_events (
    json_extract(event_json, '$.fact.delegation.action_id')
) WHERE kind = 'workhub_delegated';

CREATE UNIQUE INDEX pending_root_invocation ON message_admissions (
    json_extract(record_json, '$.invocation.invocation_id')
) WHERE json_extract(record_json, '$.source.disposition') = 'turn_started';

PRAGMA user_version = 17;
