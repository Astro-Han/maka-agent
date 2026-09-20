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
import type { CoordinationSessionAdapter } from '@maka/workhub/controller';
import type { WorkHubRootProps, WorkHubAttachmentServices, WorkHubWindowServices } from '@maka/workhub/surface';
import { useUiLocale } from '@maka/ui';
import { ClientPluginSlot, type ClientHostRef } from '../../client-plugins/index.js';
import { getDesktopConversationCopy } from '../../../locales/conversation-copy.js';
import { localizedShellErrorMessage } from '../../../locales/shell-copy.js';
import { useWorkHubServices, useWorkHubContinuation } from '../services.js';
import { hostAttachmentRefs } from '../../../../shared/desktop-session-projection.js';
import { parseDesktopSessionKey } from '../../../../shared/runtime-host-identity.js';

export function WorkHubRoot({ host, ...props }: Pick<WorkHubRootProps, 'sessionId' | 'feedback'> & { host: ClientHostRef }) {
  const locale = useUiLocale();
  const services = useWorkHubServices();
  const continuation = useWorkHubContinuation(host.hostId, props.sessionId);
  const sessionKey = useCallback((id: string) => {
    if (parseDesktopSessionKey(id).hostId !== host.hostId) throw new Error('WorkHub Session belongs to another Host');
    return id;
  }, [host.hostId]);
  const sessions = useMemo<CoordinationSessionAdapter>(() => ({
    getSession: (id) => services.getSession(sessionKey(id)),
    listSessions: async () => (await services.listSessions()).filter((session) => parseDesktopSessionKey(session.id).hostId === host.hostId),
    subscribeSessions: services.subscribeSessions,
    subscribeAvailability: services.subscribeAvailability,
    modelChoices: (id) => services.modelChoices(sessionKey(id)),
    listActiveInteractions: (id) => services.listActiveInteractions(sessionKey(id)),
    subscribeActiveInteractions: (handler) => services.subscribeActiveInteractions((event) => {
      if (parseDesktopSessionKey(event.sessionId).hostId === host.hostId) handler(event);
    }),
    respondToUserForm: (id, input) => services.respondToUserForm(sessionKey(id), input),
    respondToUserQuestion: (id, input) => services.respondToUserQuestion(sessionKey(id), input),
    retractQueueEntry: (id, entry) => services.retractQueueEntry(sessionKey(id), entry),
    promoteQueueEntry: (id, entry) => services.promoteQueueEntry(sessionKey(id), entry),
    updateQueueEntry: (id, entry, revision, text) => services.updateQueueEntry(sessionKey(id), entry, revision, text),
    reorderQueueEntries: (id, entries) => services.reorderQueueEntries(sessionKey(id), entries),
    stop: (id, turn) => services.stop(sessionKey(id), turn),
    observe: (id, ...args) => services.observe(sessionKey(id), ...args),
    openTranscript: (id, ...args) => services.openTranscript(sessionKey(id), ...args),
  }), [services, sessionKey, host.hostId]);
  const native = useMemo<WorkHubWindowServices>(() => ({
    presentation: {
      getSnapshot: () => services.presentation.getSnapshot(),
      setConversationLayout: (layout) => services.presentation.setConversationLayout(layout),
      progressReady: (request) => services.presentation.progressReady(request),
      resizeProgress: (request, height) => services.presentation.resizeProgress(request, height),
      expandProgress: (request) => services.presentation.expandProgress(request),
      detach: () => services.presentation.detach(),
      dock: () => services.presentation.dock(),
      hide: () => services.presentation.hide(),
      openUsage: () => services.presentation.openUsage(),
      toggleWorkbar: () => services.presentation.toggleWorkbar(),
      openSession: (id) => services.presentation.openSession(sessionKey(id)),
      subscribe: (handler) => services.presentation.subscribe(handler),
      onViewportInset: (handler) => services.presentation.onViewportInset(handler),
      onFocusComposer: (handler) => services.presentation.onFocusComposer(handler),
    },
    control: services.control,
    bindBrowserSession: (id) => services.bindBrowserSession(id === null ? null : sessionKey(id)),
  }), [services, sessionKey]);
  const attachments = useMemo<WorkHubAttachmentServices>(() => ({
    staging: {
      pickFiles: () => services.attachments.pickFiles(),
      previewApproval: (id) => services.attachments.previewApproval(id),
    },
    read: (id, artifact) => services.readAttachmentBytes(sessionKey(id), artifact),
    prepare: (id, items) => services.prepareAttachments(sessionKey(id), items),
    copy: (locale) => getDesktopConversationCopy(locale).actions,
    formatError: localizedShellErrorMessage,
  }), [services, sessionKey]);
  const contextUsage = useMemo<WorkHubRootProps['contextUsage']>(() => ({
    context: (id) => services.inspector.context(sessionKey(id)),
    subscribeSessionEvents: (id, handler) => services.inspector.subscribeSessionEvents(sessionKey(id), handler),
  }), [services, sessionKey]);
  const hostAttachments = useCallback<HostAttachments>((sessionId, refs) => {
    const session = parseDesktopSessionKey(sessionKey(sessionId));
    return hostAttachmentRefs({ scope: host, sessionId: session.sessionId }, refs);
  }, [host.hostId, sessionKey]);
  return <ClientPluginSlot host={host} entryId="maka.workhub.ui" name="workhub.surface" className="workHubPluginSurface"
    input={{ ...props, locale, continuation, sessions, native, attachments, hostAttachments, contextUsage }} />;
}
