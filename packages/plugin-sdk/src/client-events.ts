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

/** Canonical Session IDs on the instance's originating Host. */
export type ClientEventRequest =
  | { readonly kind: 'session.changed' }
  | { readonly kind: 'session.event' | 'tool.activity'; readonly sessionId: string };

/** UI observation, not a durable LogEvent. Seeds can replay existing IDs.
 * Event-specific payloads are open product projections, validated by consumers. */
export interface ClientObservedEvent {
  readonly id: string;
  readonly turnId: string;
  readonly ts: number;
  readonly type: string;
  readonly payload: Readonly<Record<string, unknown>>;
}

export type ClientToolEventType =
  | 'tool_start'
  | 'tool_output_delta'
  | 'tool_progress'
  | 'tool_result_preview'
  | 'tool_result';

export type ClientProductEvent =
  | {
      readonly kind: 'session.changed';
      readonly sessionId?: string;
      readonly turnId?: string;
      readonly modelId?: string;
      /** An invalidation hint, not evidence of execution completion. */
      readonly reason: string;
      readonly ts: number;
    }
  | {
      readonly kind: 'session.event';
      readonly sessionId: string;
      readonly event: ClientObservedEvent;
    }
  | {
      readonly kind: 'tool.activity';
      readonly sessionId: string;
      readonly event: ClientObservedEvent & {
        readonly type: ClientToolEventType;
        readonly payload: Readonly<Record<string, unknown>> & { readonly toolUseId: string };
      };
    };

export interface ClientEvents {
  /** Published with the instance; disposal and retirement stop delivery immediately.
   * Errors in one listener never interrupt other observers. Source failures go to onError. */
  subscribe(
    request: ClientEventRequest,
    listener: (event: ClientProductEvent) => void,
    onError?: (error: Error) => void,
  ): () => void;
}
