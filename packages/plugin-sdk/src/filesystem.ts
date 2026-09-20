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
export interface ImageBytes {
  kind: 'image';
  mimeType: string;
  bytes: Uint8Array;
}
export type Patch =
  | { type: 'create_file' | 'update_file'; path: string; diff: string }
  | { type: 'delete_file'; path: string };

export interface Files {
  /** Bounded primitives rooted in this call's granted workspace. */
  readonly entries: FileEntries;
  /** Bounded text page or image; Agent images use durable refs, other sources receive bytes. */
  read(input: ReadInput): Promise<TextPage | ImageReference | ImageBytes>;
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
export interface ReadDirectory extends ReadFiles<'follow' | 'reject'> {
  /** Observed mount location, not filesystem authority or a promise of continued identity. */
  location(): Promise<string>;
}
/** Private files never follow symlinks; input views may follow confined aliases. */
export interface ReadFiles<Links extends 'follow' | 'reject' = 'reject'> {
  /** Reads at most 1 MiB (default 64 KiB). A non-null cursor means more bytes exist. */
  read(input: { path: string; offset?: number; limit?: number; symlinks?: Links }): Promise<{
    bytes: Uint8Array;
    nextOffset: number | null;
  }>;
  /** Lexical pagination, not a snapshot across concurrent directory mutations. */
  list(input?: {
    path?: string;
    after?: string | null;
    limit?: number;
    symlinks?: Links;
  }): Promise<{
    entries: { name: string; kind: 'file' | 'directory' | 'other' }[];
    nextAfter: string | null;
  }>;
}
export interface FileEntries extends ReadFiles<never> {
  /** Flush a bounded write, not an atomic replacement. outcome_unknown requires recovery. */
  write(input: {
    path: string;
    offset?: number;
    bytes: Uint8Array | readonly number[];
    truncate?: boolean;
    createNew?: boolean;
    mode?: number;
  }): Promise<void>;
  stat(path: string): Promise<{ kind: 'file' | 'directory' | 'other'; size: number; mode: number }>;
  createDirectory(path: string): Promise<void>;
  /** Reconfirm directory durability during recovery; empty selects the root. */
  sync(path?: string): Promise<void>;
  /** Removes only a file/link or an empty directory. */
  remove(path: string): Promise<void>;
  /** Atomic move; an existing destination is never overwritten. */
  rename(from: string, to: string): Promise<void>;
}
