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

export interface SessionCatalogInput {
  readonly revision?: string;
  readonly cursor?: string;
  readonly includeArchived?: boolean;
}
export interface SessionCatalogPage {
  readonly revision: string;
  readonly entries: readonly {
    readonly session: import('./execution.js').SessionConfiguration;
    readonly archived: boolean;
    readonly labels: readonly string[];
    readonly updatedAt: number;
    readonly lastMessageAt: number | null;
  }[];
  readonly nextCursor: string | null;
}
export interface HistoryCursor {
  readonly sequence: number;
  /** UTF-8 byte offset, not a JavaScript string index. */
  readonly offset: number;
}
export interface HistoryChunk extends HistoryCursor {
  readonly messageId: string;
  readonly turnId: string;
  readonly timestamp: number;
  readonly role: 'user' | 'assistant' | 'tool_call' | 'tool_result';
  readonly totalBytes: number;
  readonly text: string;
  /** User attachment descriptors, only in the first chunk of each message. */
  readonly attachments: readonly import('./execution.js').Attachment[];
}
export type HistoryPage =
  | { readonly kind: 'preparing'; readonly through: number }
  | {
      readonly kind: 'ready';
      readonly through: number;
      readonly chunks: readonly HistoryChunk[];
      readonly next: HistoryCursor | null;
    };
export interface History {
  /** Source history access and destination execution authority are checked independently.
   * Copies an immutable user upload; retrying returns the same destination.
   */
  copyMaterial(
    target: import('./execution.js').Executions,
    input: {
      sessionId: string;
      artifactId: string;
      targetSessionId: string;
    },
  ): Promise<import('./execution.js').Attachment>;
  list(input?: SessionCatalogInput): Promise<SessionCatalogPage>;
  /** Keep through fixed across pages. Preparing repeats the same cursor.
   * Chunks contain exact UTF-8 text; next=null alone means the fence is exhausted.
   */
  read(input: {
    sessionId: string;
    through?: number;
    cursor?: HistoryCursor;
  }): Promise<HistoryPage>;
}
