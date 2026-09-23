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

/** SQL integers are decimal strings and blobs are base64; neither loses precision at the JS boundary. */
export type DatabaseCell =
  | { kind: 'null' }
  | { kind: 'integer'; value: string }
  | { kind: 'real'; value: number }
  | { kind: 'text'; value: string }
  | { kind: 'blob'; value: string };
export interface DatabaseRead {
  /** Explicit trusted Host pathname. Requires a host_paths Remote endpoint.
   * Source data is read-only; normal SQLite WAL/SHM read coordination is allowed.
   * Not a captured-directory capability against malicious same-user path replacement.
   */
  path: string;
  /** One SELECT per query, one read transaction for the entire batch.
   * Schema-inspection PRAGMAs are allowed; mutation, ATTACH and extension loading are not.
   * At most 16 queries, 250,000 rows and 64 MiB encoded result. VM work and a
   * 30-second deadline are checked cooperatively, not an OS resource sandbox.
   * Functions are limited to read-oriented scalar, aggregate and JSON inspection builtins.
   */
  queries: readonly { sql: string; parameters?: readonly DatabaseCell[] }[];
}
export interface DatabaseTable {
  columns: readonly string[];
  rows: readonly (readonly DatabaseCell[])[];
}
