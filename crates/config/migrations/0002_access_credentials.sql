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

CREATE TABLE access_credentials (
    document TEXT NOT NULL CHECK(json_valid(document) AND length(CAST(document AS BLOB)) <= 524288),
    credential_id TEXT GENERATED ALWAYS AS (json_extract(document, '$.credentialId')) STORED NOT NULL UNIQUE,
    credential_hash TEXT GENERATED ALWAYS AS (json_extract(document, '$.credentialHash')) STORED NOT NULL UNIQUE
        CHECK(length(credential_hash) = 64)
);
PRAGMA user_version = 2;
