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

CREATE TABLE frames (
    number INTEGER PRIMARY KEY CHECK(number BETWEEN 1 AND 1000000),
    record_json TEXT NOT NULL CHECK(json_valid(record_json) AND length(CAST(record_json AS BLOB)) <= 18874368),
    kind TEXT GENERATED ALWAYS AS (json_extract(record_json, '$.kind')) STORED,
    source_sequence INTEGER GENERATED ALWAYS AS (
        CASE WHEN kind = 'event' THEN json_extract(record_json, '$.sequence') END
    ) STORED
);
CREATE UNIQUE INDEX source_events ON frames(source_sequence) WHERE kind = 'event';
CREATE INDEX frame_kinds ON frames(kind, number);
CREATE UNIQUE INDEX copy_sessions ON frames(json_extract(record_json, '$.request.targetSessionId')) WHERE kind = 'copy';
CREATE UNIQUE INDEX catalog_sessions ON frames(json_extract(record_json, '$.id')) WHERE kind = 'session';
CREATE UNIQUE INDEX history_members ON frames(json_extract(record_json, '$.session'), json_extract(record_json, '$.sequence')) WHERE kind = 'member';
CREATE UNIQUE INDEX revision_sources ON frames(json_extract(record_json, '$.session'), json_extract(record_json, '$.sequence')) WHERE kind = 'revision_source';
CREATE UNIQUE INDEX event_blobs ON frames(json_extract(record_json, '$.resource'), json_extract(record_json, '$.event_id'))
    WHERE kind = 'blob' AND json_extract(record_json, '$.resource') IN ('tool_result', 'composition');
CREATE UNIQUE INDEX artifact_blobs ON frames(json_extract(record_json, '$.metadata.sessionId'), json_extract(record_json, '$.metadata.id'))
    WHERE kind = 'blob' AND json_extract(record_json, '$.resource') = 'artifact';
CREATE UNIQUE INDEX history_artifacts ON frames(json_extract(record_json, '$.session'), json_extract(record_json, '$.source_session'), json_extract(record_json, '$.source_artifact'))
    WHERE kind = 'history_artifact';

-- Derived during validation, never supplied by the transfer.
CREATE TABLE referenced_mappings (
    frame INTEGER PRIMARY KEY REFERENCES frames(number)
);

-- Keep binary payloads in bounded pieces; metadata never becomes file paths.
CREATE TABLE chunks (
    frame INTEGER NOT NULL REFERENCES frames(number),
    offset INTEGER NOT NULL CHECK(offset >= 0 AND offset % 65536 = 0),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 65536),
    PRIMARY KEY(frame, offset)
) WITHOUT ROWID;
