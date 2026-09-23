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

import type { ReactElement } from 'react';
import { Badge } from '@astryxdesign/core/Badge';
import { Button } from '@astryxdesign/core/Button';
import { EmptyState } from '@astryxdesign/core/EmptyState';
import { HStack, VStack } from '@astryxdesign/core/Stack';
import { projectLinkedSessionTree } from '@maka/core/session';
import { useUiLocale } from '@maka/ui';
import type { DesktopSessionSummary } from '../../../shared/desktop-session-projection.js';
import { getExternalSessionImportCopy } from '../../locales/external-session-import-copy.js';

/** Known catalog relationships; only the Host preview defines export membership. */
export function ExportTree(props: {
  sessions: readonly DesktopSessionSummary[];
  isBusy: boolean;
  onExport: (session: DesktopSessionSummary) => void;
}): ReactElement {
  const copy = getExternalSessionImportCopy(useUiLocale());

  // Filtered first, then projected. Deciding lineage over the whole catalog and
  // hiding rows afterwards loses a Session: an active grandchild under an
  // archived child nests under a parent that never draws it, so nobody renders
  // it at all.
  const visible = props.sessions.filter((session) => !session.isArchived);
  // The read model the rest of the app projects lineage with, rather than a
  // second one maintained here. It resolves both spellings of the link, drops a
  // parent that is not in the list, and refuses a cycle -- which `subagentParent`
  // permits, being an ordinary field with no schema guarantee behind it.
  const tree = projectLinkedSessionTree(visible);
  const childrenOf = (sessionId: string): readonly DesktopSessionSummary[] =>
    (tree.childrenByParentId.get(sessionId) ?? []) as readonly DesktopSessionSummary[];

  if (tree.roots.length === 0) return <EmptyState title={copy.exportEmpty} />;

  const node = (session: DesktopSessionSummary, depth: number): ReactElement => {
    const agent = session.subagent?.agentName ?? session.subagentRuntime?.agentName;
    const children = childrenOf(session.id);
    return (
      <li key={session.id} className="maka-export-node">
        <div className="maka-export-row">
          <VStack gap={1}>
            <HStack gap={2} vAlign="center">
              <span className="maka-export-name">{session.name ?? session.id}</span>
              {agent ? <Badge variant="neutral" label={agent} /> : null}
            </HStack>
          </VStack>
          <Button
            // A root is what someone came here to export; a child is usually
            // context. The same capability, at a quieter weight.
            variant={depth === 0 ? 'secondary' : 'ghost'}
            size="sm"
            label={copy.exportAction}
            aria-label={copy.exportActionFor(session.name ?? session.id)}
            isDisabled={props.isBusy}
            onClick={() => props.onExport(session)}
          />
        </div>
        {children.length > 0 && (
          // The rule belongs to the container, not to each row: a border on the
          // list that holds the children is exactly as tall as they are, while a
          // mark drawn beside every child draws nothing between them.
          <ul className="maka-export-subtree">
            {children.map((child) => node(child, depth + 1))}
          </ul>
        )}
      </li>
    );
  };

  return (
    <ul className="maka-export-tree" aria-label={copy.exportTitle}>
      {tree.roots.map((session) => node(session as DesktopSessionSummary, 0))}
    </ul>
  );
}
