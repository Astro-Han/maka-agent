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
import { Button } from '@astryxdesign/core/Button';
import type { QuoteRef } from '@maka/core/events';
import type { SessionQuote } from '@maka/core/session-reference';
import { useConversationServices } from '../services.js';

type Session = {
  id: string; name: string; runtimeHostId: string; isArchived: boolean; shared?: true;
};

/** Preview is explicitly attached; late reads cannot edit a different draft. */
export function SessionReferencePicker({ sessions, currentSessionId, hostId, locale, disabled, onAttach }: {
  sessions: readonly Session[];
  currentSessionId?: string;
  hostId?: string;
  locale: string;
  disabled: boolean;
  onAttach(quote: QuoteRef): void;
}) {
  const services = useConversationServices();
  const zh = locale !== 'en';
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<string>();
  const [preview, setPreview] = useState<{ sessionId: string; quote: SessionQuote }>();
  const [error, setError] = useState<string>();
  const [refresh, setRefresh] = useState(0);
  const candidates = sessions.filter((session) => session.runtimeHostId === hostId &&
    session.id !== currentSessionId && !session.isArchived && !session.shared &&
    session.name.toLowerCase().includes(query.toLowerCase()));
  const source = candidates.find((session) => session.id === selected);
  const sourceId = open ? source?.id : undefined;
  useEffect(() => {
    let active = true;
    setPreview(undefined);
    setError(undefined);
    if (sourceId) {
      void services.readSessionQuote(sourceId).then((quote) => {
        if (active) setPreview({ sessionId: sourceId, quote });
      }).catch((error: unknown) => {
        if (active) setError(error instanceof Error ? error.message : String(error));
      });
    }
    return () => { active = false; };
  }, [sourceId, services, refresh]);
  const quote = preview && preview.sessionId === sourceId ? preview.quote : undefined;
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary>{zh ? '引用会话' : 'Reference a Session'}</summary>
    {open ? <fieldset disabled={disabled}>
      <label>{zh ? '搜索当前 Host 的会话' : 'Search Sessions on this Host'}
        <input value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <label>{zh ? '来源会话' : 'Source Session'}
        <select value={source?.id ?? ''} onChange={(event) => setSelected(event.target.value)}>
          <option value="">{zh ? '选择会话' : 'Choose a Session'}</option>
          {candidates.map((session) => <option key={session.id} value={session.id}>{session.name}</option>)}
        </select>
      </label>
      {sourceId && !quote && !error ? <p role="status">{zh ? '读取快照…' : 'Reading snapshot…'}</p> : null}
      {quote ? <>
        <p>{new Date(quote.source.capturedAt).toISOString()} {quote.source.truncated ? (zh ? '（已截断）' : '(truncated)') : ''}</p>
        <pre style={{ maxHeight: 160, overflow: 'auto', whiteSpace: 'pre-wrap' }}>{quote.text}</pre>
      </> : null}
      {error ? <p role="alert">{error}</p> : null}
      <Button label={zh ? '刷新' : 'Refresh'} isDisabled={!sourceId} onClick={() => setRefresh((value) => value + 1)} />
      <Button label={zh ? '附加此快照' : 'Attach this snapshot'} isDisabled={!quote} onClick={() => {
        if (!quote) return;
        onAttach(quote);
        setOpen(false);
      }} />
    </fieldset> : null}
  </details>;
}
