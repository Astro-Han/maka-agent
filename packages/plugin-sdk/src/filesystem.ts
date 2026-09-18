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

import type { AttachmentLocation } from './execution.js';

export interface ReadInput {
  path: string;
  offset?: number;
  limit?: number;
}
export interface TextPage {
  content: string;
  offset: number;
  returnedLines: number;
  totalLines: number;
  partialLine?: boolean;
  next: ReadInput | null;
}
export interface ImageReference {
  kind: 'image';
  mimeType: string;
  ref: AttachmentLocation;
}
export type Patch =
  | { type: 'create_file' | 'update_file'; path: string; diff: string }
  | { type: 'delete_file'; path: string };

export interface Files {
  /** Bounded text page or durable image reference; pass next back unchanged. */
  read(input: ReadInput): Promise<TextPage | ImageReference>;
  /** UTF-8, at most 1 MiB. Existing parent directory required. */
  write(input: {
    path: string;
    content: string;
  }): Promise<{ kind: 'file_write'; path: string; bytes: number }>;
  edit(input: { path: string; old_string: string; new_string: string }): Promise<{
    ok: true;
    path: string;
    replacements: 1;
    matchedVia: 'exact' | 'line-trimmed' | 'whitespace' | 'escape';
    startLine: number;
    endLine: number;
  }>;
  glob(input: { pattern: string; cwd?: string }): Promise<{ files: string[]; complete: boolean }>;
  grep(input: {
    pattern: string;
    path?: string;
    glob?: string;
  }): Promise<{ matches: string[]; complete: boolean }>;
  patch(operation: Patch): Promise<{ status: 'completed' }>;
}
