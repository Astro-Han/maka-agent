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

export interface CredentialRecord {
  revision: number;
  /** null is a deletion tombstone, not a missing revision. */
  secret: string | null;
}
export interface CredentialWrite {
  key: string;
  expectedRevision: number | null;
  secret: string | null;
}
export type CredentialWriteResult =
  | { kind: 'written'; revision: number }
  | { kind: 'conflict'; actual: number | null };
export interface Credentials {
  /** Package/scope namespace is fixed by Host; secrets never enter general storage. */
  read(key: string): Promise<CredentialRecord | null>;
  /** CAS. Deleting retains its revision to prevent stale resurrection. */
  write(input: CredentialWrite): Promise<CredentialWriteResult>;
}
