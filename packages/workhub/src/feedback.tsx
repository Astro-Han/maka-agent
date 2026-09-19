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

import { useEffect } from 'react';
import type { ClientContext } from '@maka-agent/plugin-sdk/client';
import type { DelegationFeedback, DelegationReference, FeedbackInput } from './slots.js';

type Outcome =
  | { ok: true; result: DelegationFeedback[] }
  | { ok: false; error: { code: string; message: string } };

export function registerFeedback(context: ClientContext): void {
  const read = context.remote.method<DelegationReference[], Outcome>('feedback');
  context.slots.register(
    'workhub.feedback',
    'delegations',
    function Feedback(props: FeedbackInput) {
      useEffect(() => {
        const observation = new AbortController();
        void (async () => {
          const feedback: DelegationFeedback[] = [];
          // Serial bounded batches avoid one request per historical assignment.
          for (let offset = 0; offset < props.references.length; offset += 64) {
            const outcome = await read(props.references.slice(offset, offset + 64));
            if (observation.signal.aborted || context.signal.aborted) return;
            if (!outcome.ok) throw new Error(outcome.error.message);
            feedback.push(...outcome.result);
          }
          if (!observation.signal.aborted && !context.signal.aborted) props.onFeedback(feedback);
        })().catch((error: unknown) => {
          if (!observation.signal.aborted && !context.signal.aborted) {
            props.onFeedback(props.references.map(({ id }) => ({ id, state: 'recovering' })));
            props.onError(error);
          }
        });
        return () => {
          observation.abort();
        };
      }, [props.references, props.contextRevision, props.onFeedback, props.onError]);
      useEffect(() => () => props.onFeedback([]), [props.onFeedback]);
      return null;
    },
  );
}
