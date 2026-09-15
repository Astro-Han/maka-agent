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

-- Both continuations acquire the source through their canonical opening.
DROP INDEX continuation_claim_id;
DROP INDEX continuation_source_boundary;
CREATE UNIQUE INDEX continuation_claim_id ON event_log(
    json_extract(event_json, '$.fact.input.claim.id')
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') IN ('continuation','handoff');
CREATE UNIQUE INDEX continuation_source_boundary ON event_log(
    json_extract(event_json, '$.invocation.session_id'),
    json_extract(event_json, '$.fact.input.claim.source.invocation.run_id'),
    json_extract(event_json, '$.fact.input.claim.source.high_water')
) WHERE kind = 'invocation_opened' AND json_extract(event_json, '$.fact.input.kind') IN ('continuation','handoff');

-- No two paused owners can reserve the same future physical execution.
CREATE UNIQUE INDEX handoff_successor_run ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.successor_run_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';
CREATE UNIQUE INDEX handoff_successor_invocation ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.successor_invocation_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';
CREATE UNIQUE INDEX handoff_claim ON event_log(
    json_extract(event_json, '$.fact.outcome.pause.intent.claim_id')
) WHERE kind = 'invocation_ended' AND json_extract(event_json, '$.fact.outcome.kind') = 'handoff_paused';

PRAGMA user_version = 23;
