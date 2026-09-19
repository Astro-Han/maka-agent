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

import { forwardRef, useRef, useState } from 'react';
import {
  Composer,
  useToast,
  useUiLocale,
  type ComposerHandle,
  type ComposerProps,
} from '@maka/ui/plugin';
import { useComposerAttachments } from '@maka/ui/plugin';
import { toComposerIngestItems } from '@maka/ui/plugin';
import { useMountedRef } from '@maka/ui/plugin';
import { MAX_ATTACHMENT_BYTES, MAX_ATTACHMENT_COUNT } from '@maka/core/attachments';
import type { AttachmentRef, FollowUpMode } from '@maka/core/events';
import type { WorkHubAttachmentServices } from './ports.js';
import { workHubLiveCopy } from '../locales.js';

export type WorkHubComposerProps = Omit<ComposerProps, 'onSend' | 'draftKey'> & {
  sessionId?: string;
  signal?: AbortSignal;
  attachments: WorkHubAttachmentServices;
  onSend(text: string, attachments: AttachmentRef[], followUpMode?: FollowUpMode): Promise<boolean>;
};

/** The shared Composer and attachment lifecycle belong to the persistent coordination Session. */
export const WorkHubComposer = forwardRef<ComposerHandle, WorkHubComposerProps>(
  function WorkHubComposer({ sessionId, signal, attachments: service, onSend, ...composer }, ref) {
    const mounted = useMountedRef();
    const locale = useUiLocale();
    const t = workHubLiveCopy[locale];
    const toast = useToast();
    // First resolution adopts the already editable draft. Temporary Host
    // unavailability keeps it; switching Hosts selects a separate draft.
    const draftOwner = useRef({ first: sessionId, latest: sessionId });
    if (sessionId) {
      draftOwner.current.first ??= sessionId;
      draftOwner.current.latest = sessionId;
    }
    const scope = `workhub:${draftOwner.current.latest === draftOwner.current.first ? 'initial' : draftOwner.current.latest}`;
    const currentSessionId = useRef(sessionId);
    currentSessionId.current = sessionId;
    const [submitting, setSubmitting] = useState(false);
    const submittingRef = useRef(false);
    const staged = useComposerAttachments({
      draftKey: scope,
      toastApi: toast,
      service: service.staging,
      copy: service.copy(locale),
      formatError: (error, fallback) => service.formatError(error, fallback, locale),
    });
    const uploaded = useRef(new Map<string, AttachmentRef>());
    return (
      <Composer
        {...composer}
        ref={ref}
        draftKey={scope}
        sendBlocked={composer.sendBlocked || submitting || !sessionId}
        allowAttachmentOnlySend
        pendingAttachments={staged.pendingAttachments}
        onPickAttachments={staged.pickAttachments}
        onAttachFilePaths={staged.attachFilePaths}
        onRemoveAttachment={staged.removeAttachment}
        onSend={async (text, metadata) => {
          const current = () =>
            mounted.current && !signal?.aborted && currentSessionId.current === sessionId;
          if (!current() || !sessionId || submittingRef.current || composer.sendBlocked)
            return false;
          const snapshot = [...staged.pendingAttachments];
          submittingRef.current = true;
          setSubmitting(true);
          try {
            if (
              snapshot.length > MAX_ATTACHMENT_COUNT ||
              snapshot.some((item) => item.size > MAX_ATTACHMENT_BYTES)
            ) {
              throw new Error(t.attachmentLimit);
            }
            const attachments: AttachmentRef[] = [];
            for (const item of snapshot) {
              const key = `${scope}:${item.stagingKey}`;
              let attachment = uploaded.current.get(key);
              if (!attachment) {
                [attachment] = await service.prepare(sessionId, toComposerIngestItems([item]));
                if (!current()) return false;
                if (!attachment) throw new Error(t.attachmentUploadFailed);
                uploaded.current.set(key, attachment);
              }
              attachments.push(attachment);
            }
            // A Host switch during an upload must never submit the old draft into its successor.
            if (!current()) return false;
            const accepted = await onSend(
              text.trim() || t.reviewAttachments,
              attachments,
              metadata?.followUpMode,
            );
            if (!current()) return false;
            if (accepted) {
              staged.clearSubmittedAttachments(snapshot);
              for (const item of snapshot) uploaded.current.delete(`${scope}:${item.stagingKey}`);
            }
            return accepted;
          } catch (error) {
            if (current()) toast.error(t.sendFailed, service.formatError(error, t.retry, locale));
            return false;
          } finally {
            submittingRef.current = false;
            if (mounted.current) setSubmitting(false);
          }
        }}
      />
    );
  },
);
