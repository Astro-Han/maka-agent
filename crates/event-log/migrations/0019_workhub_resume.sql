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


DROP INDEX workhub_action_identity;
CREATE UNIQUE INDEX workhub_action_identity ON runtime_events (
    CASE
        WHEN kind = 'workhub_delegated'
            THEN json_extract(event_json, '$.fact.delegation.action_id')
        WHEN kind = 'workhub_resume_observed'
            THEN json_extract(event_json, '$.fact.resume.action_id')
        WHEN kind = 'invocation_opened'
            AND json_extract(event_json, '$.fact.input.kind') = 'continuation'
            THEN json_extract(event_json, '$.fact.input.workhub_resume.action_id')
    END
) WHERE kind IN ('workhub_delegated', 'workhub_resume_observed', 'invocation_opened');

PRAGMA user_version = 19;
