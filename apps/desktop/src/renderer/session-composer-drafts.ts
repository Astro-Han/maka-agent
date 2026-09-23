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

import type { PendingAttachment } from './composer-attachments.js';
import { attachmentKindFromMimeType } from '@maka/core/attachments';
import type { DesktopComposerDraft, DesktopComposerDraftRecord, DesktopSessionLocalBridge } from '../shared/session-local-contract.js';

type Upload = NonNullable<Parameters<DesktopSessionLocalBridge['saveDraft']>[3]>[number];
export type DraftSubmissionIdentity = { messageId: string; draftVersion: number; draftAuthority: string };
type Entry = {
  record: DesktopComposerDraftRecord;
  snapshot: DesktopComposerDraft;
  uploads: Map<string, Upload>;
  changes: number;
  saved: number;
  rebasedAt: number;
  tail: Promise<unknown>;
  timer?: ReturnType<typeof setTimeout>;
  uncertain?: { messageId: string; version: number; snapshot: DesktopComposerDraft };
};

const empty = (): DesktopComposerDraft => ({ text: '', attachments: [] });

/** Editing buffer only. Main owns persistence and the single draft→outbox handoff. */
export class SessionComposerDrafts {
  readonly #entries = new Map<string, Entry>();
  readonly #loading = new Map<string, Promise<Entry>>();

  constructor(private readonly bridge: Pick<DesktopSessionLocalBridge, 'readDraft' | 'readDraftFile' | 'saveDraft'>,
    private readonly onError: (error: unknown) => void) {}

  async load(sessionId: string): Promise<DesktopComposerDraft> {
    return (await this.#entry(sessionId)).snapshot;
  }

  async refresh(sessionId: string): Promise<DesktopComposerDraft> {
    const entry = await this.#entry(sessionId);
    const record = await this.bridge.readDraft(sessionId);
    if (this.#entries.get(sessionId) !== entry) throw new Error('Draft load was superseded');
    if (record.authority !== entry.record.authority) {
      this.forget(sessionId);
      this.#entries.set(sessionId, newEntry(record));
    }
    return this.#entries.get(sessionId)!.snapshot;
  }

  read(sessionId: string | undefined): string | undefined {
    return sessionId ? this.#entries.get(sessionId)?.snapshot.text : undefined;
  }

  snapshot(sessionId: string): DesktopComposerDraft | undefined {
    return this.#entries.get(sessionId)?.snapshot;
  }

  forget(sessionId: string): void {
    const entry = this.#entries.get(sessionId);
    if (entry) clearTimeout(entry.timer);
    this.#entries.delete(sessionId);
    this.#loading.delete(sessionId);
  }

  write(sessionId: string | undefined, text: string, workspaceFileReferences?: DesktopComposerDraft['workspaceFileReferences']): void {
    if (sessionId) this.update(sessionId, { text, ...(workspaceFileReferences === undefined ? {} : { workspaceFileReferences }) });
  }

  update(sessionId: string, patch: Partial<DesktopComposerDraft>, uploads: readonly Upload[] = []): void {
    const entry = this.#entries.get(sessionId);
    if (!entry) return; // The editor stays disabled until its complete draft is loaded.
    const snapshot = { ...entry.snapshot, ...patch };
    for (const upload of uploads) entry.uploads.set(upload.id, upload);
    if (JSON.stringify(snapshot) === JSON.stringify(entry.snapshot)) return;
    entry.snapshot = snapshot;
    entry.changes += 1;
    clearTimeout(entry.timer);
    entry.timer = setTimeout(() => { void this.flush(sessionId).catch(this.onError); }, 200);
  }

  stage(sessionId: string, pending: readonly PendingAttachment[], patch: Partial<DesktopComposerDraft>): void {
    const uploads: Upload[] = [];
    const attachments = pending.map((item) => {
      if (item.source.type === 'retained') return { id: item.stagingKey, kind: 'retained' as const, attachment: item.source.attachment };
      uploads.push({ id: item.stagingKey, item: item.source.type === 'file' ? { file: item.source.file } : {
        approvalId: item.source.approvalId, name: item.source.name, mimeType: item.mimeType,
      } });
      return { id: item.stagingKey, kind: 'file' as const, name: item.displayName,
        mimeType: item.mimeType ?? 'application/octet-stream', bytes: item.size };
    });
    this.update(sessionId, { ...patch, attachments }, uploads);
  }

  async attachments(sessionId: string): Promise<PendingAttachment[]> {
    await this.flush(sessionId);
    const snapshot = await this.load(sessionId);
    return Promise.all(snapshot.attachments.map(async (item): Promise<PendingAttachment> => {
      if (item.kind === 'retained') return { stagingKey: item.id, displayName: item.attachment.name,
        kind: item.attachment.kind, mimeType: item.attachment.mimeType, size: item.attachment.bytes,
        source: { type: 'retained', attachment: item.attachment } };
      const stored = await this.bridge.readDraftFile(sessionId, item.id, this.#entries.get(sessionId)!.record.authority);
      const bytes = Uint8Array.from(atob(stored.base64), (value) => value.charCodeAt(0));
      const file = new File([bytes], stored.name, { type: stored.mimeType });
      return { stagingKey: item.id, displayName: file.name, kind: attachmentKindFromMimeType(file.type, file.name),
        mimeType: file.type, size: file.size, source: { type: 'file', file } };
    }));
  }

  async flush(sessionId: string): Promise<void> {
    const entry = this.#entries.get(sessionId);
    if (!entry) { await this.#entry(sessionId); return this.flush(sessionId); }
    const captured = { snapshot: entry.snapshot, changes: entry.changes };
    await this.#serial(entry, () => this.#save(sessionId, entry, captured));
  }

  async flushAll(): Promise<void> {
    await Promise.all([...this.#entries.keys()].map((key) => this.flush(key)));
  }

  async seed(sessionId: string, snapshot: DesktopComposerDraft): Promise<DesktopComposerDraft> {
    const entry = await this.#entry(sessionId);
    return this.#serial(entry, async () => {
      // A previously saved or consumed editor must never be reseeded from the
      // original Turn, even if creation's reply was lost before navigation.
      if (entry.record.version === 0 && entry.changes === 0) {
        entry.snapshot = snapshot;
        entry.changes += 1;
        await this.#save(sessionId, entry);
      }
      return entry.snapshot;
    });
  }

  async submit(sessionId: string, send: (identity: DraftSubmissionIdentity) => Promise<boolean>): Promise<boolean> {
    const entry = this.#entries.get(sessionId);
    if (!entry) throw new Error('Load the complete draft before submitting');
    // Freeze at the click, before waiting for any earlier autosave. The wire
    // command and attachment membership must describe this same editor version.
    const captured = { snapshot: entry.snapshot, changes: entry.changes };
    return this.#serial(entry, async () => {
      if (entry.uncertain) throw new Error('The previous draft submission is unresolved; reload before sending again');
      await this.#save(sessionId, entry, captured);
      const submission = { messageId: crypto.randomUUID(), version: entry.record.version, snapshot: captured.snapshot };
      entry.uncertain = submission;
      try {
        // Even a lost IPC reply is resolved by this exact consumed version, not
        // by another send or by the continued existence of an outbox row.
        await send({ messageId: submission.messageId, draftVersion: submission.version, draftAuthority: entry.record.authority });
      } catch (error) {
        this.onError(error);
      }
      {
        const record = await this.bridge.readDraft(sessionId);
        if (record.authority !== entry.record.authority || this.#entries.get(sessionId) !== entry)
          throw new Error('Draft authority changed during submission');
        if (record.submittedMessageId === submission.messageId && record.version === submission.version + 1) {
          entry.record = record;
          const current = entry.snapshot;
          const captured = submission.snapshot;
          entry.snapshot = { ...current,
            text: current.text === captured.text ? '' : current.text,
            attachments: current.attachments.filter((item) => !captured.attachments.some((old) => old.id === item.id)),
            quotes: current.quotes?.filter((item) => !captured.quotes?.includes(item)),
            directoryReferences: current.directoryReferences?.filter((item) => !captured.directoryReferences?.includes(item)),
            workspaceFileReferences: current.workspaceFileReferences?.filter((item) => !captured.workspaceFileReferences?.includes(item)),
            revision: undefined, inputSelections: undefined, turnOrchestration: undefined,
          };
          // Autosaves queued during the handoff captured the old submitted
          // items too. They are superseded; only the rebased successor may save.
          entry.saved = entry.changes;
          entry.changes += 1;
          entry.rebasedAt = entry.changes;
          entry.uncertain = undefined;
          // The caller clears only submitted UI items. Do not wait for a second
          // save to acknowledge a handoff that is already durable.
          clearTimeout(entry.timer);
          entry.timer = setTimeout(() => { void this.flush(sessionId).catch(this.onError); }, 0);
        } else if (record.version === submission.version && !record.submittedMessageId) {
          entry.uncertain = undefined;
          return false;
        } else throw new Error('Composer draft changed during submission; reload it before continuing');
      }
      return true;
    });
  }

  async #entry(sessionId: string): Promise<Entry> {
    const existing = this.#entries.get(sessionId);
    if (existing) return existing;
    let loading = this.#loading.get(sessionId);
    if (!loading) {
      loading = this.bridge.readDraft(sessionId).then((record) => {
        if (this.#loading.get(sessionId) !== loading) throw new Error('Draft load was retired');
        const entry = newEntry(record);
        this.#entries.set(sessionId, entry);
        return entry;
      }).finally(() => { if (this.#loading.get(sessionId) === loading) this.#loading.delete(sessionId); });
      this.#loading.set(sessionId, loading);
    }
    return loading;
  }

  async #save(sessionId: string, entry: Entry,
    captured = { snapshot: entry.snapshot, changes: entry.changes }): Promise<void> {
    clearTimeout(entry.timer);
    if (this.#entries.get(sessionId) !== entry) throw new Error('Draft belongs to a retired editor');
    if (entry.uncertain) throw new Error('Draft submission is unresolved; reload before saving further changes');
    if (captured.changes < entry.rebasedAt) captured = { snapshot: entry.snapshot, changes: entry.changes };
    if (entry.saved !== captured.changes) {
      const { changes, snapshot } = captured;
      const existing = new Set(entry.record.snapshot?.attachments.map((item) => item.id));
      const uploads = [...entry.uploads.values()].filter((upload) => !existing.has(upload.id) && snapshot.attachments.some((item) => item.id === upload.id));
      const record = await this.bridge.saveDraft(sessionId, entry.record.version, snapshot, uploads, entry.record.authority);
      if (this.#entries.get(sessionId) !== entry || record.authority !== entry.record.authority)
        throw new Error('Draft save belongs to a retired authority');
      entry.record = record;
      entry.saved = changes;
      for (const item of entry.record.snapshot?.attachments ?? []) entry.uploads.delete(item.id);
    }
    if (entry.saved !== entry.changes)
      entry.timer = setTimeout(() => { void this.flush(sessionId).catch(this.onError); }, 200);
  }

  #serial<T>(entry: Entry, operation: () => Promise<T>): Promise<T> {
    const task = entry.tail.catch(() => undefined).then(operation);
    entry.tail = task;
    return task;
  }
}

function newEntry(record: DesktopComposerDraftRecord): Entry {
  return { record, snapshot: record.snapshot ?? empty(), uploads: new Map(),
    changes: 0, saved: 0, rebasedAt: 0, tail: Promise.resolve() };
}
