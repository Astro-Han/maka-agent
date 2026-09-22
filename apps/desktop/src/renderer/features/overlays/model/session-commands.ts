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

import type { UiLocale } from '@maka/core/ui-locale';
import type { SessionSummary } from '@maka/core/session';
import { Palette, MessageSquare } from '@maka/ui/icons';
import { getShellCopy } from '../../../locales/shell-copy.js';
import type { Command } from './command.js';

export function buildSessionCommands(args: {
  locale: UiLocale;
  sessions: readonly SessionSummary[];
  activeSessionId: string | undefined;
  onSelectSession(id: string): void;
}): Command[] {
  const copy = getShellCopy(args.locale).commandPalette;
  const cmds: Command[] = [];
  for (const session of args.sessions) {
    if (session.isArchived) continue;
    cmds.push({
      id: `session:${session.id}`,
      kind: 'session',
      label: session.name,
      hint: session.id === args.activeSessionId ? copy.current : undefined,
      group: copy.groups.conversations,
      Icon: session.isFlagged ? Palette : MessageSquare,
      keywords: ['session', 'chat', session.name],
      run: () => args.onSelectSession(session.id),
    });
  }
  return cmds;
}
