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

import { chmodSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { dirname } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import type {
  SessionCreateInput,
  TurnMessageSubmitInput,
  TurnMessageSubmitResult,
} from '@maka/runtime-host/protocol';
import type { DesktopSessionSummaryInput } from '../shared/desktop-session-projection.js';
import type { DesktopComposerDraft, DesktopComposerDraftRecord, DesktopLocalMessageState } from '../shared/session-local-contract.js';
import type { DesktopTranscriptReplicaSnapshot } from './desktop-transcript-replica.js';

const MAX_OUTBOX_BYTES = 256 * 1024 * 1024;
const MAX_DRAFT_BYTES = 256 * 1024 * 1024;
export const MAX_LOCAL_MESSAGE_BYTES = 64 * 1024 * 1024;
const MAX_CACHE_BYTES = 64 * 1024 * 1024;
const MAX_CACHE_SESSION_BYTES = 2 * 1024 * 1024;
const CACHE_TTL_MS = 30 * 24 * 60 * 60 * 1000;

export interface LocalStagedAttachment {
  readonly name: string;
  readonly mimeType: string;
  readonly base64: string;
}

export interface LocalDraftAttachment extends LocalStagedAttachment {
  readonly id: string;
}

export interface LocalMessageIntent {
  readonly command: Omit<TurnMessageSubmitInput, 'originHostEpoch'>;
  readonly staged: readonly LocalStagedAttachment[];
  /** Written durably before the first dispatch, and immutable thereafter. */
  readonly originHostEpoch?: string;
  readonly attachmentsPrepared?: true;
}

export interface LocalOutboxRecord {
  readonly partition: string;
  readonly sessionId: string;
  readonly messageId: string;
  readonly createdAt: number;
  readonly state: DesktopLocalMessageState;
  readonly intent: Omit<LocalMessageIntent, 'staged'>;
  readonly fingerprint: string;
  readonly result?: TurnMessageSubmitResult;
  readonly error?: string;
}

/** A Client-owned database, never the Host's operational database. */
export class DesktopSessionLocalStore {
  readonly #db: DatabaseSync;
  #revision = 0;
  get revision(): number {
    return this.#revision;
  }
  constructor(
    path: string,
    private readonly now: () => number = Date.now,
  ) {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    this.#db = new DatabaseSync(path);
    chmodSync(path, 0o600);
    this.#db.exec(`
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      PRAGMA foreign_keys = ON;
      PRAGMA busy_timeout = 5000;
    `);
    this.#migrate();
    // A crash may have happened anywhere after persisting dispatch intent.
    // Recovery probes the original epoch instead of assuming the send failed.
    this.#db.exec("UPDATE outbox SET state = 'unknown' WHERE state = 'sending'");
    this.#db.prepare('DELETE FROM transcripts WHERE updated_at < ?').run(now() - CACHE_TTL_MS);
  }

  #migrate(): void {
    const migrations = [String.raw`
      CREATE TABLE IF NOT EXISTS outbox (
        partition TEXT NOT NULL, session_id TEXT NOT NULL, message_id TEXT NOT NULL,
        created_at INTEGER NOT NULL, state TEXT NOT NULL, payload TEXT NOT NULL,
        PRIMARY KEY (partition, message_id)
      );
      CREATE INDEX IF NOT EXISTS outbox_order ON outbox(partition, created_at);
      CREATE TABLE IF NOT EXISTS outbox_attachments (
        partition TEXT NOT NULL, message_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
        name TEXT NOT NULL, mime_type TEXT NOT NULL, content BLOB NOT NULL,
        PRIMARY KEY (partition, message_id, ordinal),
        FOREIGN KEY (partition, message_id) REFERENCES outbox(partition, message_id) ON DELETE CASCADE
      );
      CREATE TABLE IF NOT EXISTS sessions (
        partition TEXT NOT NULL, session_id TEXT NOT NULL, summary TEXT NOT NULL,
        creation TEXT, updated_at INTEGER NOT NULL,
        PRIMARY KEY (partition, session_id)
      );
      CREATE TABLE IF NOT EXISTS transcripts (
        partition TEXT NOT NULL, session_id TEXT NOT NULL, snapshot TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (partition, session_id)
      );
      CREATE TABLE IF NOT EXISTS authorities (profile_id TEXT PRIMARY KEY, partition TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS composer_drafts (
        partition TEXT NOT NULL, session_id TEXT NOT NULL,
        version INTEGER NOT NULL CHECK (version > 0), snapshot TEXT,
        submitted_message_id TEXT, submission_fingerprint TEXT,
        PRIMARY KEY (partition, session_id)
      );
      CREATE TABLE IF NOT EXISTS composer_draft_attachments (
        partition TEXT NOT NULL, session_id TEXT NOT NULL, attachment_id TEXT NOT NULL,
        name TEXT NOT NULL, mime_type TEXT NOT NULL, content BLOB NOT NULL,
        PRIMARY KEY (partition, session_id, attachment_id),
        FOREIGN KEY (partition, session_id) REFERENCES composer_drafts(partition, session_id) ON DELETE CASCADE
      );
    `];
    this.#transaction(() => {
      const version = Number(this.#db.prepare('PRAGMA user_version').get()!.user_version);
      if (version > migrations.length) throw new Error('Desktop database was created by a newer version');
      for (let index = version; index < migrations.length; index += 1) {
        this.#db.exec(migrations[index]!);
        this.#db.exec(`PRAGMA user_version = ${index + 1}`);
      }
    });
  }

  close(): void {
    this.#db.close();
  }

  bindAuthority(profileId: string, partition: string): void {
    const previous = this.#db
      .prepare('SELECT partition FROM authorities WHERE profile_id = ?')
      .get(profileId);
    if (previous?.partition === partition) return;
    if (previous) this.purge(String(previous.partition));
    this.#db
      .prepare(
        'INSERT INTO authorities VALUES (?, ?) ON CONFLICT(profile_id) DO UPDATE SET partition = excluded.partition',
      )
      .run(profileId, partition);
  }

  draft(partition: string, sessionId: string): DesktopComposerDraftRecord {
    const row = this.#db.prepare(
      'SELECT version, snapshot, submitted_message_id FROM composer_drafts WHERE partition = ? AND session_id = ?',
    ).get(partition, sessionId);
    return row ? {
      authority: partition,
      version: Number(row.version),
      snapshot: row.snapshot === null ? null : JSON.parse(String(row.snapshot)) as DesktopComposerDraft,
      ...(row.submitted_message_id ? { submittedMessageId: String(row.submitted_message_id) } : {}),
    } : { authority: partition, version: 0, snapshot: null };
  }

  /** CAS covers the complete editor state, including attachment membership. */
  saveDraft(
    partition: string, sessionId: string, expectedVersion: number,
    snapshot: DesktopComposerDraft | null, files: readonly LocalDraftAttachment[] = [],
  ): DesktopComposerDraftRecord {
    return this.#transaction(() => {
      const previous = this.draft(partition, sessionId);
      const payload = snapshot === null ? null : JSON.stringify(snapshot);
      if (previous.version !== expectedVersion) {
        // A lost save acknowledgement is not a new edit. A consumed version can
        // never match this branch, even if an old autosave tries to clear it.
        if (previous.version === expectedVersion + 1 && !previous.submittedMessageId &&
            JSON.stringify(previous.snapshot) === JSON.stringify(snapshot) && files.length === 0)
          return previous;
        throw new Error('Composer draft changed; reload it before saving');
      }
      const attachments = snapshot?.attachments.filter((item) => item.kind === 'file') ?? [];
      if (new Set(snapshot?.attachments.map((item) => item.id)).size !== (snapshot?.attachments.length ?? 0))
        throw new Error('Duplicate draft attachment identity');
      if (new Set(files.map((item) => item.id)).size !== files.length ||
          files.some((file) => !attachments.some((item) => item.id === file.id)))
        throw new Error('Draft contains an unreferenced attachment upload');
      this.#db.prepare(`INSERT INTO composer_drafts VALUES (?, ?, ?, ?, NULL, NULL)
        ON CONFLICT(partition, session_id) DO UPDATE SET version = excluded.version,
          snapshot = excluded.snapshot, submitted_message_id = NULL, submission_fingerprint = NULL`)
        .run(partition, sessionId, expectedVersion + 1, payload);
      const insert = this.#db.prepare('INSERT INTO composer_draft_attachments VALUES (?, ?, ?, ?, ?, ?)');
      for (const attachment of attachments) {
        const stored = this.draftAttachment(partition, sessionId, attachment.id);
        const upload = files.find((item) => item.id === attachment.id);
        const content = upload ? Buffer.from(upload.base64, 'base64') : stored?.content;
        if (!content || content.byteLength !== attachment.bytes ||
            attachment.name !== (upload?.name ?? stored?.name) ||
            attachment.mimeType !== (upload?.mimeType ?? stored?.mimeType))
          throw new Error('Draft attachment snapshot is missing or changed');
        if (stored) {
          if (stored.name !== attachment.name || stored.mimeType !== attachment.mimeType ||
              !Buffer.from(stored.content).equals(content))
            throw new Error('Draft attachment identity is already bound to different bytes');
        } else insert.run(partition, sessionId, attachment.id, attachment.name, attachment.mimeType, content);
      }
      for (const row of this.#db.prepare(
        'SELECT attachment_id FROM composer_draft_attachments WHERE partition = ? AND session_id = ?',
      ).all(partition, sessionId)) {
        if (!attachments.some((item) => item.id === row.attachment_id))
          this.#db.prepare('DELETE FROM composer_draft_attachments WHERE partition = ? AND session_id = ? AND attachment_id = ?')
            .run(partition, sessionId, row.attachment_id!);
      }
      const metadataBytes = Number(this.#db.prepare(
        'SELECT COALESCE(SUM(length(CAST(snapshot AS BLOB))), 0) AS bytes FROM composer_drafts',
      ).get()!.bytes);
      const fileBytes = Number(this.#db.prepare(
        'SELECT COALESCE(SUM(length(content)), 0) AS bytes FROM composer_draft_attachments',
      ).get()!.bytes);
      if (Buffer.byteLength(payload ?? '') + attachments.reduce((sum, item) => sum + item.bytes, 0) > MAX_LOCAL_MESSAGE_BYTES ||
          metadataBytes + fileBytes > MAX_DRAFT_BYTES)
        throw new Error('Local draft storage is full; no drafts were discarded');
      return this.draft(partition, sessionId);
    });
  }

  draftAttachment(partition: string, sessionId: string, id: string):
    { name: string; mimeType: string; content: Uint8Array } | undefined {
    const row = this.#db.prepare(
      'SELECT name, mime_type, content FROM composer_draft_attachments WHERE partition = ? AND session_id = ? AND attachment_id = ?',
    ).get(partition, sessionId, id);
    return row ? { name: String(row.name), mimeType: String(row.mime_type), content: row.content as Uint8Array } : undefined;
  }

  /** Transfer ownership once. The fence outlives outbox retirement and ACK loss. */
  submitDraft(
    partition: string, expectedVersion: number,
    command: LocalMessageIntent['command'],
  ): { messageId: string; record?: LocalOutboxRecord } {
    return this.#transaction(() => {
      const previous = this.draft(partition, command.sessionId);
      const fingerprint = createHash('sha256').update(JSON.stringify(command)).digest('hex');
      if (previous.version === expectedVersion + 1 && previous.submittedMessageId === command.messageId) {
        const row = this.#db.prepare(
          'SELECT submission_fingerprint FROM composer_drafts WHERE partition = ? AND session_id = ?',
        ).get(partition, command.sessionId)!;
        if (row.submission_fingerprint !== fingerprint)
          throw new Error('Message identity is already bound to a different draft submission');
        return { messageId: command.messageId, record: this.get(partition, command.messageId) };
      }
      if (previous.version !== expectedVersion || !previous.snapshot)
        throw new Error('Composer draft changed or was already submitted');
      const staged = previous.snapshot.attachments.flatMap((attachment) => {
        if (attachment.kind !== 'file') return [];
        const file = this.draftAttachment(partition, command.sessionId, attachment.id);
        if (!file) throw new Error('Draft attachment snapshot is missing');
        return [{ name: file.name, mimeType: file.mimeType, base64: Buffer.from(file.content).toString('base64') }];
      });
      const record = this.#enqueue(partition, { command, staged });
      this.#db.prepare(`UPDATE composer_drafts SET version = version + 1, snapshot = NULL,
        submitted_message_id = ?, submission_fingerprint = ? WHERE partition = ? AND session_id = ?`)
        .run(command.messageId, fingerprint, partition, command.sessionId);
      this.#db.prepare('DELETE FROM composer_draft_attachments WHERE partition = ? AND session_id = ?')
        .run(partition, command.sessionId);
      return { messageId: command.messageId, record };
    });
  }

  enqueue(partition: string, intent: LocalMessageIntent): LocalOutboxRecord {
    return this.#transaction(() => this.#enqueue(partition, intent));
  }

  #enqueue(partition: string, intent: LocalMessageIntent): LocalOutboxRecord {
    const fingerprint = intentFingerprint(intent);
    const previous = this.get(partition, intent.command.messageId);
    if (previous) {
      // Only an identical caller retry can claim a durable local receipt.
      if (previous.fingerprint !== fingerprint) {
        throw new Error('Message identity is already bound to a different local intent');
      }
      return previous;
    }
    const { staged, ...metadata } = intent;
    const record: LocalOutboxRecord = {
      partition,
      sessionId: intent.command.sessionId,
      messageId: intent.command.messageId,
      createdAt: this.now(),
      state: 'saved',
      intent: metadata,
      fingerprint,
    };
    const payload = JSON.stringify(record);
    const usage = this.#db
      .prepare(
        'SELECT COUNT(*) AS count, COALESCE(SUM(length(CAST(payload AS BLOB))), 0) AS bytes FROM outbox',
      )
      .get()!;
    const storedBytes = Number(
      this.#db
        .prepare('SELECT COALESCE(SUM(length(content)), 0) AS bytes FROM outbox_attachments')
        .get()!.bytes,
    );
    const messageBytes =
      Buffer.byteLength(payload) +
      staged.reduce((bytes, item) => bytes + Buffer.byteLength(item.base64, 'base64'), 0);
    if (
      Number(usage.count) >= 256 ||
      messageBytes > MAX_LOCAL_MESSAGE_BYTES ||
      Number(usage.bytes) + storedBytes + messageBytes > MAX_OUTBOX_BYTES
    ) {
      throw new Error(
        'Local message storage is full; keep the draft and resolve pending messages first',
      );
    }
    this.#db
        .prepare('INSERT INTO outbox VALUES (?, ?, ?, ?, ?, ?)')
        .run(
          partition,
          record.sessionId,
          record.messageId,
          record.createdAt,
          record.state,
          payload,
        );
    const insert = this.#db.prepare('INSERT INTO outbox_attachments VALUES (?, ?, ?, ?, ?, ?)');
    staged.forEach((item, ordinal) =>
        insert.run(
          partition,
          record.messageId,
          ordinal,
          item.name,
          item.mimeType,
          Buffer.from(item.base64, 'base64'),
        ),
    );
    this.#revision += 1;
    return record;
  }

  /** Only the delivering worker reads blobs; catalog/UI/retry scans read metadata. */
  stagedAttachments(
    partition: string,
    messageId: string,
  ): { name: string; mimeType: string; content: Uint8Array }[] {
    return this.#db
      .prepare(
        'SELECT name, mime_type, content FROM outbox_attachments WHERE partition = ? AND message_id = ? ORDER BY ordinal',
      )
      .all(partition, messageId)
      .map((row) => ({
        name: String(row.name),
        mimeType: String(row.mime_type),
        content: row.content as Uint8Array,
      }));
  }

  get(partition: string, messageId: string): LocalOutboxRecord | undefined {
    const row = this.#db
      .prepare('SELECT state, payload FROM outbox WHERE partition = ? AND message_id = ?')
      .get(partition, messageId);
    return row
      ? ({ ...JSON.parse(String(row.payload)), state: row.state } as LocalOutboxRecord)
      : undefined;
  }

  list(partition: string, sessionId?: string): LocalOutboxRecord[] {
    const rows =
      sessionId === undefined
        ? this.#db
            .prepare(
              'SELECT state, payload FROM outbox WHERE partition = ? ORDER BY created_at, rowid',
            )
            .all(partition)
        : this.#db
            .prepare(
              'SELECT state, payload FROM outbox WHERE partition = ? AND session_id = ? ORDER BY created_at, rowid',
            )
            .all(partition, sessionId);
    return rows.map(
      (row) => ({ ...JSON.parse(String(row.payload)), state: row.state }) as LocalOutboxRecord,
    );
  }

  update(record: LocalOutboxRecord): void {
    const previous = this.get(record.partition, record.messageId);
    if (!previous) throw new Error('Local intent was removed');
    if (
      previous.intent.originHostEpoch &&
      previous.intent.originHostEpoch !== record.intent.originHostEpoch
    )
      throw new Error('Cannot retarget a dispatched Message epoch');
    this.#transaction(() => {
      this.#db
        .prepare('UPDATE outbox SET state = ?, payload = ? WHERE partition = ? AND message_id = ?')
        .run(record.state, JSON.stringify(record), record.partition, record.messageId);
      // Host references and local bytes change ownership in the same commit.
      if (record.intent.attachmentsPrepared)
        this.#db
          .prepare('DELETE FROM outbox_attachments WHERE partition = ? AND message_id = ?')
          .run(record.partition, record.messageId);
    });
  }

  cancel(partition: string, messageId: string): void {
    const record = this.get(partition, messageId);
    if (!record) return;
    if ((record.intent.originHostEpoch && record.state !== 'failed') || record.state === 'accepted')
      throw new Error(
        'The Host may already own this message; local cancellation cannot stop execution',
      );
    this.#db
      .prepare('DELETE FROM outbox WHERE partition = ? AND message_id = ?')
      .run(partition, messageId);
  }

  saveSession(
    partition: string,
    summary: DesktopSessionSummaryInput,
    creation?: SessionCreateInput,
  ): void {
    this.#db
      .prepare(`INSERT INTO sessions VALUES (?, ?, ?, ?, ?)
      ON CONFLICT(partition, session_id) DO UPDATE SET summary = excluded.summary,
        creation = excluded.creation, updated_at = excluded.updated_at`)
      .run(
        partition,
        summary.id,
        JSON.stringify(summary),
        creation ? JSON.stringify(creation) : null,
        this.now(),
      );
    this.#revision += 1;
  }

  sessions(partition: string): DesktopSessionSummaryInput[] {
    return this.#db
      .prepare('SELECT summary FROM sessions WHERE partition = ? ORDER BY updated_at DESC')
      .all(partition)
      .map((row) => JSON.parse(String(row.summary)) as DesktopSessionSummaryInput);
  }

  creation(partition: string, sessionId: string): SessionCreateInput | undefined {
    const row = this.#db
      .prepare('SELECT creation FROM sessions WHERE partition = ? AND session_id = ?')
      .get(partition, sessionId);
    return row?.creation ? (JSON.parse(String(row.creation)) as SessionCreateInput) : undefined;
  }

  session(partition: string, sessionId: string): DesktopSessionSummaryInput | undefined {
    const row = this.#db.prepare('SELECT summary FROM sessions WHERE partition = ? AND session_id = ?').get(partition, sessionId);
    return row ? JSON.parse(String(row.summary)) as DesktopSessionSummaryInput : undefined;
  }

  saveCatalog(partition: string, summaries: readonly DesktopSessionSummaryInput[]): void {
    this.#transaction(() => {
      const seen = new Set(summaries.map((summary) => summary.id));
      for (const summary of summaries) this.saveSession(partition, summary);
      for (const previous of this.sessions(partition)) {
        if (!seen.has(previous.id) && !this.creation(partition, previous.id))
          this.#removeSession(partition, previous.id);
      }
    });
  }

  retireObservedMessages(partition: string, snapshot: DesktopTranscriptReplicaSnapshot): boolean {
    const pending = new Set(
      this.#db
        .prepare(
          "SELECT message_id FROM outbox WHERE partition = ? AND session_id = ? AND state IN ('sending', 'unknown', 'accepted')",
        )
        .all(partition, snapshot.sessionId)
        .map((row) => String(row.message_id)),
    );
    if (!pending.size) return false;
    const observed = snapshot.durable.filter(
      (entry) => entry.message.type === 'user' && pending.has(entry.message.id),
    );
    if (!observed.length) return false;
    return this.#transaction(() => {
      let changed = false;
      const remove = this.#db.prepare(
        "DELETE FROM outbox WHERE partition = ? AND session_id = ? AND message_id = ? AND state IN ('sending', 'unknown', 'accepted')",
      );
      for (const entry of observed) {
        if (remove.run(partition, snapshot.sessionId, entry.message.id).changes) changed = true;
      }
      return changed;
    });
  }

  saveTranscript(partition: string, snapshot: DesktopTranscriptReplicaSnapshot): void {
    const payload = JSON.stringify(snapshot);
    if (Buffer.byteLength(payload) > MAX_CACHE_SESSION_BYTES) return;
    this.#transaction(() => {
      this.#db
        .prepare(`INSERT INTO transcripts VALUES (?, ?, ?, ?)
        ON CONFLICT(partition, session_id) DO UPDATE SET snapshot = excluded.snapshot, updated_at = excluded.updated_at`)
        .run(partition, snapshot.sessionId, payload, this.now());
      let total = Number(
        this.#db
          .prepare(
            'SELECT COALESCE(SUM(length(CAST(snapshot AS BLOB))), 0) AS bytes FROM transcripts',
          )
          .get()!.bytes,
      );
      for (const row of this.#db
        .prepare(
          'SELECT rowid, length(CAST(snapshot AS BLOB)) AS bytes FROM transcripts ORDER BY updated_at, rowid',
        )
        .all()) {
        if (total <= MAX_CACHE_BYTES) break;
        this.#db.prepare('DELETE FROM transcripts WHERE rowid = ?').run(row.rowid!);
        total -= Number(row.bytes);
      }
    });
  }

  transcript(
    partition: string,
    sessionId: string,
  ): { snapshot: DesktopTranscriptReplicaSnapshot; cachedAt: number } | undefined {
    const row = this.#db
      .prepare(
        'SELECT snapshot, updated_at FROM transcripts WHERE partition = ? AND session_id = ? AND updated_at >= ?',
      )
      .get(partition, sessionId, this.now() - CACHE_TTL_MS);
    return row
      ? {
          snapshot: JSON.parse(String(row.snapshot)) as DesktopTranscriptReplicaSnapshot,
          cachedAt: Number(row.updated_at),
        }
      : undefined;
  }

  removeSession(partition: string, sessionId: string): void {
    this.#transaction(() => this.#removeSession(partition, sessionId));
  }

  #removeSession(partition: string, sessionId: string): void {
    for (const table of ['outbox', 'sessions', 'transcripts', 'composer_drafts'])
      this.#db.prepare(`DELETE FROM ${table} WHERE partition = ? AND session_id = ?`).run(partition, sessionId);
    this.#revision += 1;
  }

  purge(partition: string): void {
    this.#transaction(() => {
      for (const table of ['outbox', 'sessions', 'transcripts', 'composer_drafts'])
        this.#db.prepare(`DELETE FROM ${table} WHERE partition = ?`).run(partition);
    });
    this.#revision += 1;
  }

  #transaction<T>(operation: () => T): T {
    this.#db.exec('BEGIN IMMEDIATE');
    try {
      const result = operation();
      this.#db.exec('COMMIT');
      return result;
    } catch (error) {
      this.#db.exec('ROLLBACK');
      throw error;
    }
  }
}

function intentFingerprint(intent: LocalMessageIntent): string {
  const digest = createHash('sha256').update(JSON.stringify(intent.command));
  for (const staged of intent.staged)
    digest.update(JSON.stringify([staged.name, staged.mimeType, staged.base64]));
  return digest.digest('hex');
}
