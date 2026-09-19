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

import type {} from '@maka-agent/plugin-sdk/client';

export type DelegationReference = {
  readonly id: string;
  /** Canonical Session ID on the Client's originating Host. */
  readonly targetSessionId: string;
  readonly targetMessageId: string;
};

export type DelegationFeedback = {
  readonly id: string;
  readonly state:
    | 'accepted'
    | 'running'
    | 'waiting_for_user'
    | 'completed'
    | 'failed'
    | 'aborted'
    | 'recovering';
  readonly resultPreview?: string;
};

export interface FeedbackInput {
  readonly locale: 'en' | 'zh-CN' | 'zh-TW';
  readonly contextRevision?: number;
  readonly references: readonly DelegationReference[];
  readonly onFeedback: (feedback: readonly DelegationFeedback[]) => void;
  readonly onError: (error: unknown) => void;
}

declare module '@maka-agent/plugin-sdk/client' {
  interface ClientSlots {
    'workhub.feedback': FeedbackInput;
  }
}
