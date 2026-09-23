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

import { z } from 'zod';
import { isAttachmentRef, isDirectoryReference, isQuoteRef, type AttachmentRef, type DirectoryReference, type QuoteRef } from '@maka/core/events';
import { isOrchestrationMode, TURN_ORCHESTRATION_SOURCES } from '@maka/core/orchestration';
import { decodeInputSelections } from '@maka/runtime-host/protocol';
import type { DesktopComposerDraft } from './session-local-contract.js';

const id = z.string().min(1).max(256);
export const composerDraftVersion = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER - 1);

/** Structural recovery validation; submitting still applies native admission limits. */
export const composerDraftSchema = z.strictObject({
  text: z.string(),
  attachments: z.array(z.discriminatedUnion('kind', [
    z.strictObject({ id, kind: z.literal('retained'), attachment: z.custom<AttachmentRef>(isAttachmentRef) }),
    z.strictObject({ id, kind: z.literal('file'), name: z.string().min(1), mimeType: z.string().min(1), bytes: z.number().int().nonnegative() }),
  ])),
  directoryReferences: z.array(z.custom<DirectoryReference>(isDirectoryReference)).optional(),
  quotes: z.array(z.custom<QuoteRef>(isQuoteRef)).optional(),
  workspaceFileReferences: z.array(z.strictObject({ value: z.string().min(1), start: z.number().int().nonnegative() })).optional(),
  inputSelections: z.record(z.string(), z.array(z.string())).transform((value, ctx) => {
    try { return decodeInputSelections(value); }
    catch { ctx.addIssue({ code: 'custom', message: 'Invalid input selections' }); return z.NEVER; }
  }).optional(),
  turnOrchestration: z.strictObject({ mode: z.string().refine(isOrchestrationMode), source: z.enum(TURN_ORCHESTRATION_SOURCES) }).optional(),
  revision: z.strictObject({ sourceSessionId: id, sourceTurnId: id, copyId: id, phase: z.enum(['preparing', 'ready', 'abandoning']) }).optional(),
}) satisfies z.ZodType<DesktopComposerDraft>;

export const composerDraftSaveSchema = z.strictObject({
  authority: id,
  expectedVersion: composerDraftVersion,
  snapshot: composerDraftSchema.nullable(),
  uploads: z.array(z.strictObject({ id, item: z.unknown() })),
});
