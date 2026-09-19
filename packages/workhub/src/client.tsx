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

import { useEffect, useState } from 'react';
import { Button } from '@maka/ui/plugin';
import type { ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';
import { registerFeedback } from './feedback.js';

type Resolution =
  | { ok: true; result: { sessionId: string } }
  | { ok: false; error: { code: string; message: string } };

const plugin: ClientPlugin = {
  activate(context) {
    registerFeedback(context);
    const resolve = context.remote.method<null, Resolution>('resolve');
    context.slots.register(
      'session.resolve',
      'coordinator',
      function Coordinator(props: ClientSlots['session.resolve']) {
        const [failed, setFailed] = useState(false);
        const [retry, setRetry] = useState(0);
        useEffect(() => {
          props.onResolving();
          return props.onResolving;
        }, [props.onResolving]);
        useEffect(() => {
          const observation = new AbortController();
          setFailed(false);
          void resolve(null)
            .then((outcome) => {
              if (observation.signal.aborted) return;
              if (!outcome.ok) throw new Error(outcome.error.message);
              props.onResolved(outcome.result.sessionId, observation.signal);
            })
            .catch((error: unknown) => {
              if (!observation.signal.aborted) {
                setFailed(true);
                props.onError(error instanceof Error ? error.message : String(error));
              }
            });
          return () => {
            observation.abort();
          };
        }, [props.contextRevision, props.onResolved, props.onError, retry]);
        return failed ? (
          <Button
            label={props.locale === 'en' ? 'Retry' : props.locale === 'zh-TW' ? '重試' : '重试'}
            onClick={() => {
              props.onResolving();
              setRetry((current) => current + 1);
            }}
          />
        ) : null;
      },
    );
  },
};
export default plugin;
