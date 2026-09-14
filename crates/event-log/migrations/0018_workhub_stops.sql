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


CREATE TABLE workhub_stops (
    action_id TEXT PRIMARY KEY NOT NULL,
    delegation_action_id TEXT NOT NULL,
    record_json TEXT NOT NULL CHECK(json_valid(record_json)),
    resolution_json TEXT CHECK(resolution_json IS NULL OR json_valid(resolution_json))
);
CREATE INDEX workhub_stop_subject ON workhub_stops(delegation_action_id);
CREATE UNIQUE INDEX workhub_stop_claim ON workhub_stops(delegation_action_id)
    WHERE resolution_json IS NULL OR json_extract(resolution_json, '$.outcome') != 'not_owned';
PRAGMA user_version = 18;
