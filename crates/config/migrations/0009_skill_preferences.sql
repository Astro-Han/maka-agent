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

-- Root-owned preferences are control state, not model history or imported TS data.
CREATE TABLE skill_preferences (
    ref TEXT PRIMARY KEY NOT NULL CHECK (length(CAST(ref AS BLOB)) BETWEEN 1 AND 512),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    pinned INTEGER NOT NULL CHECK (pinned IN (0, 1))
) STRICT;
CREATE TABLE skill_preferences_revision (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 9007199254740991)
) STRICT;
INSERT INTO skill_preferences_revision VALUES (1, 0);
PRAGMA user_version = 9;

