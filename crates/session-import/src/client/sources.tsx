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

import { useState } from 'react';
import type { Snapshot, Source } from './model.js';
import { path } from './model.js';

export function Sources({
  snapshot,
  busy,
  zh,
  save,
}: {
  snapshot: Snapshot;
  busy: boolean;
  zh: boolean;
  save: (sources: Source[]) => void;
}) {
  const [kind, setKind] = useState<Source['location']['kind']>('codex');
  const [name, setName] = useState('');
  const [location, setLocation] = useState('');
  return (
    <details>
      <summary>{zh ? '管理导入来源' : 'Manage import sources'}</summary>
      <p>
        {zh
          ? '路径属于当前 Host，不是桌面客户端。来源仅用于只读导入。'
          : 'Paths belong to this Host, not the desktop client. Sources are read only.'}
      </p>
      <ul>
        {snapshot.configuration.sources.map((source) => (
          <li key={source.id}>
            <strong>{source.name}</strong> <code>{path(source)}</code>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                save(snapshot.configuration.sources.filter((item) => item.id !== source.id))
              }
            >
              {zh ? '移除来源' : 'Remove source'}
            </button>
          </li>
        ))}
      </ul>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          save([
            ...snapshot.configuration.sources,
            {
              id: crypto.randomUUID(),
              name: name.trim(),
              location:
                kind === 'open_code'
                  ? { kind, database: location.trim() }
                  : { kind, root: location.trim() },
            },
          ]);
        }}
      >
        <label>
          {zh ? '格式' : 'Format'}
          <select
            value={kind}
            disabled={busy}
            onChange={(event) => setKind(event.target.value as typeof kind)}
          >
            <option value="codex">Codex</option>
            <option value="claude_code">Claude Code</option>
            <option value="open_code">OpenCode</option>
          </select>
        </label>
        <label>
          {zh ? '名称' : 'Name'}
          <input
            required
            maxLength={256}
            value={name}
            disabled={busy}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          {kind === 'open_code'
            ? zh
              ? 'Host 数据库绝对路径'
              : 'Absolute Host database path'
            : zh
              ? 'Host 来源根目录'
              : 'Host source root'}
          <input
            required
            value={location}
            disabled={busy}
            onChange={(event) => setLocation(event.target.value)}
          />
        </label>
        <button
          type="submit"
          disabled={
            busy || !name.trim() || !location.trim() || snapshot.configuration.sources.length >= 16
          }
        >
          {zh ? '添加来源' : 'Add source'}
        </button>
      </form>
    </details>
  );
}
