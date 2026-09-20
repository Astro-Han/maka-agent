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

export type FormField = {
  name: string;
  label: string;
  required: boolean;
  description?: string;
} & (
  | {
      kind: 'string';
      default?: string;
      minLength?: number;
      maxLength?: number;
      format?: 'email' | 'uri' | 'date' | 'date-time';
    }
  | { kind: 'number' | 'integer'; default?: number; minimum?: number; maximum?: number }
  | { kind: 'boolean'; default?: boolean }
  | {
      kind: 'single_select';
      options: readonly { value: string; label: string }[];
      default?: string;
    }
  | {
      kind: 'multi_select';
      options: readonly { value: string; label: string }[];
      default?: readonly string[];
      minItems?: number;
      maxItems?: number;
    }
);
export type InteractionPrompt =
  | {
      kind: 'question';
      questions: readonly {
        question: string;
        options: readonly { label: string; description?: string }[];
      }[];
    }
  | { kind: 'form'; message: string; fields: readonly FormField[] };
export type InteractionOutcome = { committedAt: number } & (
  | { kind: 'question_answer'; answers: readonly (string | null)[] }
  | ({ kind: 'form_answer' } & (
      | {
          action: 'accept';
          values: Readonly<Record<string, string | number | boolean | readonly string[]>>;
        }
      | { action: 'decline' | 'cancel' }
    ))
  | {
      kind: 'closure';
      reason:
        | 'turn_stopped'
        | 'turn_terminal'
        | 'producer_cancelled'
        | 'timed_out'
        | 'host_restarted'
        | 'provider_disconnected';
    }
);
export interface Interaction {
  sessionId: string;
  turnId: string;
  runId: string;
  requestId: string;
  createdAt: number;
  request: (
    | Extract<InteractionPrompt, { kind: 'question' }>
    | (Extract<InteractionPrompt, { kind: 'form' }> & {
        requester: { name: string; source?: string };
      })
  ) & { toolUseId: string };
  outcome: InteractionOutcome | null;
}
