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

import type { ToolResultContent } from '@maka/core/events';
import { projectAgentSwarmResult } from '@maka/core/agent-swarm';
import { ptyHumanTerminalText } from '@maka/core/pty-output-view';
import { type ShellOutput } from '@maka/core/shell-run';

export function formatToolResultContent(content: ToolResultContent): string {
  switch (content.kind) {
    case 'text':
      return content.text;
    case 'json':
      return formatUnknown(content.value);
    case 'terminal':
      return [
        `$ ${content.cmd}`,
        `cwd: ${content.cwd}`,
        `status: ${content.status}`,
        content.exitCode !== undefined ? `exit: ${content.exitCode}` : '',
        formatShellOutput(content.output),
      ]
        .filter(Boolean)
        .join('\n\n');
    case 'shell_run':
      return [
        `$ ${content.cmd}`,
        `cwd: ${content.cwd}`,
        `ref: ${content.ref}`,
        `status: ${content.status}`,
        content.exitCode !== undefined ? `exit: ${content.exitCode}` : '',
        content.output ? formatShellOutput(content.output) : '',
      ]
        .filter(Boolean)
        .join('\n\n');
    case 'file_diff':
      return content.diff;
    case 'file_write':
      return `Wrote ${content.bytes} bytes to ${content.path}`;
    case 'summary':
      return content.summarized;
    case 'image':
      return `${content.mimeType} image result`;
    case 'web_search':
      return [
        `Search ${content.provider}: ${content.query}`,
        ...content.rows.map((row) => `${row.title}\n${row.url}\n${row.snippet}`),
      ].join('\n\n');
    case 'web_search_error':
      return content.message;
    case 'subagent':
      return content.summary;
    case 'agent_swarm': {
      const projection = projectAgentSwarmResult(content);
      return limitText(
        [
          [
            `Agent swarm: ${projection.status}`,
            `${projection.itemCount} items`,
            `${projection.completedItemCount} completed`,
            `${projection.failedItemCount} failed`,
            `${projection.cancelledItemCount} cancelled`,
            `${projection.artifactCount} artifacts`,
            `${projection.durationMs}ms`,
          ].join(' · '),
          ...content.items.map((item) =>
            [
              [
                `${item.itemId}: ${item.status}`,
                item.profile,
                item.durationMs !== undefined ? `${item.durationMs}ms` : '',
                `${item.artifactIds.length} artifacts`,
                item.resumedFromRunId ? `resumed from ${item.resumedFromRunId}` : '',
                item.runId ? `run ${item.runId}` : '',
                item.turnId ? `turn ${item.turnId}` : '',
                item.failureClass ?? '',
              ]
                .filter(Boolean)
                .join(' · '),
              limitText(item.summary, 1_000),
            ].join('\n'),
          ),
        ].join('\n\n'),
        16_000,
      );
    }
    case 'rive_workflow':
      return content.summary;
    case 'archived_tool_result':
      return `Archived tool result: ${content.status}`;
  }
}

function formatShellOutput(output: ShellOutput): string {
  if (output.mode === 'pty') {
    const terminal = ptyHumanTerminalText(output);
    return terminal ? `terminal:\n${terminal}` : '';
  }
  return [
    output.stdout ? `stdout:\n${output.stdout}` : '',
    output.stderr ? `stderr:\n${output.stderr}` : '',
  ]
    .filter(Boolean)
    .join('\n\n');
}

function formatUnknown(value: unknown): string {
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function limitText(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  return `${text.slice(0, maxChars)}\n... ${text.length - maxChars} chars truncated`;
}
