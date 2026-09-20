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

CREATE TABLE connection_catalog (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision >= 0),
    default_target TEXT
);
INSERT INTO connection_catalog VALUES(1, 0, NULL);
CREATE TABLE connections (
    connection_id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    document TEXT NOT NULL
);
CREATE TABLE credential_vault (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision >= 0)
);
INSERT INTO credential_vault VALUES(1, 0);
CREATE TABLE credentials (
    locator TEXT PRIMARY KEY,
    credential_id TEXT NOT NULL UNIQUE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    secret TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE access_credentials (
    document TEXT NOT NULL CHECK(json_valid(document) AND length(CAST(document AS BLOB)) <= 524288),
    credential_id TEXT GENERATED ALWAYS AS (json_extract(document, '$.credentialId')) STORED NOT NULL UNIQUE,
    credential_hash TEXT GENERATED ALWAYS AS (json_extract(document, '$.credentialHash')) STORED NOT NULL UNIQUE
        CHECK(length(credential_hash) = 64)
);

CREATE TABLE runtime_policy (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    document TEXT NOT NULL CHECK(
        json_valid(document) AND length(CAST(document AS BLOB)) <= 49152
    )
);

-- Receipts survive logout/removal: they prove completion, not current authorization.
CREATE TABLE oauth_login_receipts (
    completion_order INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id TEXT NOT NULL UNIQUE CHECK(length(attempt_id) BETWEEN 1 AND 128),
    target TEXT NOT NULL,
    connection TEXT NOT NULL
);

CREATE TABLE plugin_credentials (
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    key TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    secret TEXT CHECK(secret IS NULL OR length(CAST(secret AS BLOB)) <= 65536),
    PRIMARY KEY (package_id, scope_id, key)
) STRICT;

CREATE TABLE plugin_authorizations (
    id TEXT PRIMARY KEY NOT NULL,
    package_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    document TEXT NOT NULL CHECK(length(CAST(document AS BLOB)) <= 65536),
    revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1)),
    UNIQUE(package_id, scope_id, operation_id)
) STRICT;

PRAGMA user_version = 1;
