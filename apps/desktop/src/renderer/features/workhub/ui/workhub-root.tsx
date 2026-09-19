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

import { useCallback, useMemo } from 'react';
import type { HostAttachments } from '@maka/workhub/slots';
import type { WorkHubRootProps, WorkHubAttachmentServices, WorkHubWindowServices } from '@maka/workhub/surface';
import { useUiLocale } from '@maka/ui';
import { ClientPluginSlot, type ClientHostRef } from '../../client-plugins/index.js';
import { getDesktopConversationCopy } from '../../../locales/conversation-copy.js';
import { localizedShellErrorMessage } from '../../../locales/shell-copy.js';
import { useWorkHubServices } from '../services.js';
import { hostAttachmentRefs } from '../../../../shared/desktop-session-projection.js';
import { parseDesktopSessionKey } from '../../../../shared/runtime-host-identity.js';

export function WorkHubRoot({ host, ...props }: Pick<WorkHubRootProps, 'sessionId' | 'feedback'> & { host: ClientHostRef }) {
  const locale = useUiLocale();
  const services = useWorkHubServices();
  const native = useMemo<WorkHubWindowServices>(() => ({
    presentation: services.presentation,
    control: services.control,
    bindBrowserSession: services.bindBrowserSession,
  }), [services]);
  const attachments = useMemo<WorkHubAttachmentServices>(() => ({
    staging: services.attachments,
    read: services.readAttachmentBytes,
    prepare: services.prepareAttachments,
    copy: (locale) => getDesktopConversationCopy(locale).actions,
    formatError: localizedShellErrorMessage,
  }), [services]);
  const hostAttachments = useCallback<HostAttachments>((sessionId, refs) => {
    const session = parseDesktopSessionKey(sessionId);
    if (session.hostId !== host.hostId) throw new Error('WorkHub Session belongs to another Host');
    return hostAttachmentRefs({ scope: host, sessionId: session.sessionId }, refs);
  }, [host.hostId]);
  return <ClientPluginSlot host={host} entryId="maka.workhub.ui" name="workhub.surface" className="workHubPluginSurface"
    input={{ ...props, locale, sessions: services, native, attachments, hostAttachments, contextUsage: services.inspector }} />;
}
