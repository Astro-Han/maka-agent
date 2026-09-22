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

import { Button } from '@astryxdesign/core';
import type { PermissionsRequestEvent } from '@maka/core/events';
import type { PermissionDecision, PermissionsResponse } from '@maka/core/execution-permissions';
import { useId, useRef, useState } from 'react';
import { getConversationCopy } from './conversation-copy.js';
import { useUiLocale } from './locale-context.js';
import { useMountedRef } from './use-mounted-ref.js';

export interface PermissionsPromptProps {
  request: PermissionsRequestEvent;
  onRespond(response: PermissionsResponse): void | Promise<void>;
}

/** A new canonical request gets fresh selections, including the shortest scope. */
export function PermissionsPrompt(props: PermissionsPromptProps) {
  return <Request key={props.request.requestId} {...props} />;
}

function Request({request, onRespond}: PermissionsPromptProps) {
  const locale = useUiLocale();
  const copy = getConversationCopy(locale).sandboxBoundary;
  const labels = COPY[locale];
  const titleId = useId();
  const mounted = useMountedRef();
  const pendingRef = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string>();
  const [scope, setScope] = useState<'once' | 'turn' | 'session'>(request.toolUseId === null ? 'turn' : 'once');
  const [readOnly, setReadOnly] = useState(false);
  const [network, setNetwork] = useState(request.request.permissions.network !== 'denied');
  const access = request.request.permissions;

  async function respond(allow: boolean) {
    if (pendingRef.current) return;
    pendingRef.current = true;
    setPending(true);
    setError(undefined);
    const decision: PermissionDecision = allow ? {
      decision: 'allow', scope, permissions: {
        filesystem: access.filesystem.map(rule => ({...rule, access: readOnly ? 'read' : rule.access})),
        network: network ? access.network : 'denied',
      },
    } : {decision: 'deny'};
    try {
      await onRespond({requestId: request.requestId, decision});
    } catch (error) {
      if (mounted.current) setError(String(error));
    } finally {
      pendingRef.current = false;
      if (mounted.current) setPending(false);
    }
  }

  return <section className="maka-composer-interaction maka-permissions-prompt composer" aria-labelledby={titleId}>
    <div className="maka-composer-interaction-inner">
      <h2 id={titleId}>{labels.title}</h2>
      <p>{request.request.reason}</p>
      {request.request.command && <>
        <pre><code>{request.request.command.command}</code></pre>
        <p><strong>{labels.cwd}</strong> <code>{request.request.command.cwd}</code></p>
      </>}
      <ul>{access.filesystem.map((rule, index) =>
        <li key={index}><code>{rule.path}</code> — {copy.access[readOnly ? 'read' : rule.access]} · {copy.scope[rule.scope]}</li>,
      )}</ul>
      <fieldset disabled={pending}>
        <legend>{labels.access}</legend>
        {access.filesystem.some(rule => rule.access === 'write') &&
          <label><input type="checkbox" checked={readOnly} onChange={event => setReadOnly(event.target.checked)} />{labels.readOnly}</label>}
        {typeof access.network === 'object' && <ul>
          {access.network.restricted.destinations.map(({host, port}) =>
            <li key={JSON.stringify([host, port])}><code>{host.includes(':') ? `[${host}]` : host}:{port}</code></li>)}
        </ul>}
        {access.network !== 'denied' &&
          <label><input type="checkbox" checked={network} onChange={event => setNetwork(event.target.checked)} />{copy.network}</label>}
        <label>{labels.duration}<select value={scope} onChange={event => {
          const scope = event.target.value;
          if (scope === 'once' || scope === 'turn' || scope === 'session') setScope(scope);
        }}>
          {request.toolUseId !== null && <option value="once">{labels.once}</option>}
          <option value="turn">{labels.turn}</option>
          <option value="session">{labels.session}</option>
        </select></label>
      </fieldset>
      {error && <p role="alert">{error}</p>}
      <div className="maka-sandbox-boundary-actions">
        <Button variant="secondary" isDisabled={pending} label={copy.reject} onClick={() => void respond(false)} />
        <Button variant="primary" isDisabled={pending} label={labels.allow} onClick={() => void respond(true)} />
      </div>
    </div>
  </section>;
}

const COPY = {
  'en': {title: 'Additional permissions', cwd: 'Working directory', access: 'Approve access',
    readOnly: 'Read-only access', duration: 'Duration', once: 'This command only',
    turn: 'This turn', session: 'This session', allow: 'Allow selected access'},
  'zh-CN': {title: '额外权限', cwd: '工作目录', access: '批准的权限',
    readOnly: '仅允许读取', duration: '有效期', once: '仅本次命令',
    turn: '当前轮次', session: '当前会话', allow: '允许所选权限'},
  'zh-TW': {title: '額外權限', cwd: '工作目錄', access: '核准的權限',
    readOnly: '僅允許讀取', duration: '有效期', once: '僅本次命令',
    turn: '目前回合', session: '目前工作階段', allow: '允許所選權限'},
};
