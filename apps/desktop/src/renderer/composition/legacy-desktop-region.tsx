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

import { useEffect, useMemo, type ComponentProps } from 'react';
import type { FeedbackInput } from '@maka/workhub/slots';
import { useUiLocale } from '@maka/ui';
import { ClientPluginSlot, usePluginSession, type ClientHostRef } from '../features/client-plugins/index.js';
import { parseDesktopSessionKey } from '../../shared/runtime-host-identity.js';
import { AppShell as LegacyAppShell } from '../app-shell';
import { WorkHubRoot, WorkHubSurfaceSwitch } from '../features/workhub';
export function AppShell(props: ComponentProps<typeof LegacyAppShell>) {
  return <WorkHubSurfaceSwitch main={<LegacyAppShell {...props} />} workhub={<WorkHubConversation />} />;
}
function WorkHubConversation() {
  const binding = usePluginSession('maka.workhub.ui', true, useUiLocale());
  return <>{binding.resolver}{binding.host ? <WorkHubRoot host={binding.host} key={binding.sessionId} sessionId={binding.sessionId}
    feedback={(input) => <WorkHubFeedback host={binding.host!} input={input} />} /> : null}</>;
}

function WorkHubFeedback({ host, input }: { host: ClientHostRef; input: FeedbackInput }) {
  const converted = useMemo(() => {
    try {
      return { references: input.references.map((reference) => {
        const target = parseDesktopSessionKey(reference.targetSessionId);
        if (target.hostId !== host.hostId) throw new Error('Delegation belongs to another Host');
        return { ...reference, targetSessionId: target.sessionId };
      }) };
    } catch (error) { return { error }; }
  }, [host.hostId, input.references]);
  useEffect(() => {
    if ('error' in converted) { input.onFeedback([]); input.onError(converted.error); }
  }, [converted, input.onFeedback, input.onError]);
  return converted.references ? <ClientPluginSlot host={host} entryId="maka.workhub.ui"
    name="workhub.feedback" input={{ ...input, references: converted.references }} /> : null;
}
