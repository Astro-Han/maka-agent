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
import type { ClientContext, ClientPlugin, ClientSlots } from '@maka-agent/plugin-sdk/client';

type Item = { content: string; status: 'pending' | 'in_progress' | 'completed' };
type Page = { revision: number | null; offset: number; total: number; items: Item[] };

function Checklist({
  context,
  sessionId,
  locale,
}: ClientSlots['session.composer.before'] & { context: ClientContext }) {
  const [snapshot, setSnapshot] = useState<{ sessionId: string; items: Item[] } | null>(null);
  const [failure, setFailure] = useState<{ sessionId: string; message: string } | null>(null);
  const [attempt, retry] = useState(0);
  const zh = locale !== 'en';
  useEffect(() => {
    const stop = new AbortController();
    setFailure(null);
    void (async () => {
      let items: Item[] = [];
      let revision: number | null = null;
      let total = 0;
      for await (const page of context.remote.stream<null, Page>('watch', sessionId)(
        null,
        stop.signal,
      )) {
        if (stop.signal.aborted || context.signal.aborted) return;
        if (page.offset === 0) {
          items = [];
          revision = page.revision;
          total = page.total;
        }
        if (
          page.revision !== revision ||
          page.total !== total ||
          page.offset !== items.length ||
          total < 0 ||
          total > 200 ||
          items.length + page.items.length > total
        ) {
          throw new Error('Inconsistent checklist snapshot');
        }
        items.push(...page.items);
        if (items.length === total) setSnapshot({ sessionId, items: [...items] });
      }
    })().catch((reason) => {
      if (!stop.signal.aborted && !context.signal.aborted)
        setFailure({ sessionId, message: String(reason) });
    });
    return () => stop.abort();
  }, [context, sessionId, attempt]);

  if (failure?.sessionId === sessionId)
    return (
      <div className="maka-todo" role="status">
        <span>{failure.message}</span>
        <button type="button" onClick={() => retry((value) => value + 1)}>
          {zh ? '重试' : 'Retry'}
        </button>
      </div>
    );
  if (snapshot?.sessionId !== sessionId || snapshot.items.length === 0) return null;
  const complete = snapshot.items.filter((item) => item.status === 'completed').length;
  return (
    <details className="maka-todo">
      <summary>
        {zh ? '待办' : 'Checklist'} · {complete}/{snapshot.items.length}
      </summary>
      <p>
        {zh
          ? '状态由模型报告，不代表执行结果已验证。'
          : 'Model-reported progress, not verified execution evidence.'}
      </p>
      <ol>
        {snapshot.items.map((item, index) => (
          <li key={index} data-status={item.status}>
            <span aria-label={item.status}>
              {item.status === 'completed' ? '✓' : item.status === 'in_progress' ? '◐' : '○'}
            </span>
            <span>{item.content}</span>
          </li>
        ))}
      </ol>
    </details>
  );
}

const plugin: ClientPlugin = {
  activate(context) {
    context.slots.register(
      'session.composer.before',
      'checklist',
      (props) => <Checklist {...props} context={context} />,
      { order: 20 },
    );
    context.style(`
      .maka-todo { font-size: 12px; padding: 6px 8px; }
      .maka-todo summary { cursor: pointer; }
      .maka-todo p { opacity: .65; margin: 6px 0; }
      .maka-todo ol { max-height: 240px; overflow-y: auto; margin: 0; padding: 0; list-style: none; }
      .maka-todo li { display: flex; gap: 6px; padding: 3px 0; overflow-wrap: anywhere; }
      .maka-todo li[data-status="completed"] { opacity: .6; }
    `);
  },
};
export default plugin;
