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
import type { ClientPlugin } from '@maka-agent/plugin-sdk/client';

const plugin: ClientPlugin = {
  activate(context) {
    const queue = context.remote.method<{ grant: string; operation: string }, boolean>('queue');
    const read = context.remote.method<null, { grant?: string } | null>('state');
    context.slots.register('application.manage', 'workflow', function Workflow() {
      const [state, setState] = useState<{ grant?: string } | null>(null);
      const [error, setError] = useState('');
      const [busy, setBusy] = useState(false);
      useEffect(() => {
        let closed = false;
        const update = async () => {
          try {
            const result = await read(null);
            if (!closed) setState(result);
          } catch (error) {
            if (!closed) setError(String(error));
          }
        };
        void update();
        const timer = setInterval(() => void update(), 250);
        return () => {
          closed = true;
          clearInterval(timer);
        };
      }, []);
      return (
        <section aria-label="Public plugin acceptance">
          <h2>Public plugin workflow</h2>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError('');
              void (async () => {
                const grant = await context.authorization.approve('profile', {
                  operationId: crypto.randomUUID(),
                  title: 'Run a public plugin workflow',
                  target: { kind: 'plugin_workspace', permissionMode: 'explore' },
                  capabilities: ['executions'],
                });
                if (grant && !grant.revoked)
                  await queue({ grant: grant.id, operation: crypto.randomUUID() });
              })()
                .catch((error) => setError(String(error)))
                .finally(() => setBusy(false));
            }}
          >
            Authorize and run
          </button>
          <button
            type="button"
            disabled={!state?.grant || busy}
            onClick={() => {
              if (!state?.grant) return;
              setBusy(true);
              void context.authorization
                .revoke('profile', state.grant)
                .catch((error) => setError(String(error)))
                .finally(() => setBusy(false));
            }}
          >
            Revoke authorization
          </button>
          <pre aria-label="Workflow result">{JSON.stringify(state, null, 2)}</pre>
          {error ? <p role="alert">{error}</p> : null}
        </section>
      );
    });
  },
};
export default plugin;
