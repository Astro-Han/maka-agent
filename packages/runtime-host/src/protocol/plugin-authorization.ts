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

import type {
  AuthorizationCapability,
  AuthorizationGrant,
  AuthorizationRequest,
  AuthorizationScope,
  ClientIdentity,
} from '@maka-agent/plugin-sdk/client';
import {
  requireEncodedByteLimit,
  requireExactRecord,
  requireId,
  requireRecord,
  requireString,
} from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';
import { decodePluginClientIdentity } from './plugin-remote.js';

export interface PluginAuthorizationInput {
  client: ClientIdentity;
  scope: AuthorizationScope;
  command:
    | { kind: 'approve'; request: AuthorizationRequest }
    | { kind: 'query' | 'revoke'; id: string };
}
export type PluginAuthorizationResult =
  | { kind: 'grant'; grant: AuthorizationGrant | null }
  | { kind: 'revoked' };
const capabilities: readonly AuthorizationCapability[] = [
  'read_files',
  'write_files',
  'network',
  'models',
  'processes',
  'client_capabilities',
  'executions',
  'notifications',
];

export const PLUGIN_AUTHORIZATION_OPERATION_SPECS = {
  'plugin.authorization': defineOperation<
    PluginAuthorizationInput,
    PluginAuthorizationResult,
    | 'host_not_ready'
    | 'host_draining'
    | 'invalid_request'
    | 'unauthorized'
    | 'operation_conflict'
    | 'operation_unavailable'
    | 'persistence_failed'
    | 'commit_outcome_unknown'
    | 'internal_failure'
  >({
    mode: 'command',
    availability: 'ready',
    errors: [
      'host_not_ready',
      'host_draining',
      'invalid_request',
      'unauthorized',
      'operation_conflict',
      'operation_unavailable',
      'persistence_failed',
      'commit_outcome_unknown',
      'internal_failure',
    ],
    decodeInput(value) {
      requireEncodedByteLimit(value, 'Plugin authorization', 64 * 1024);
      const row = requireExactRecord(value, 'Plugin authorization', ['client', 'scope', 'command']);
      const client = decodePluginClientIdentity(row.client);
      const rawScope = requireString(row.scope, 'Authorization scope', 256);
      let scope: AuthorizationScope;
      if (rawScope === 'profile') scope = rawScope;
      else if (rawScope.startsWith('session:'))
        scope = `session:${requireId(rawScope.slice(8), 'Authorization Session')}`;
      else throw invalidProtocolFrame('Invalid authorization scope');
      const command = requireRecord(row.command, 'Authorization command');
      switch (command.kind) {
        case 'approve': {
          requireExactRecord(command, 'Approve authorization', ['kind', 'request']);
          const request = proposal(command.request);
          if (
            scope !== 'profile' &&
            (request.target.kind !== 'session' || `session:${request.target.sessionId}` !== scope)
          )
            throw invalidProtocolFrame('Authorization escapes its Session scope');
          return { client, scope, command: { kind: 'approve', request } };
        }
        case 'query':
        case 'revoke':
          requireExactRecord(command, 'Authorization reference', ['kind', 'id']);
          return { client, scope, command: { kind: command.kind, id: uuid(command.id) } };
        default:
          throw invalidProtocolFrame('Invalid authorization command');
      }
    },
    decodeOutput(value) {
      requireEncodedByteLimit(value, 'Plugin authorization result', 64 * 1024);
      const row = requireRecord(value, 'Authorization result');
      if (row.kind === 'revoked') {
        requireExactRecord(row, 'Revoked authorization', ['kind']);
        return { kind: 'revoked' };
      }
      requireExactRecord(row, 'Authorization grant', ['kind', 'grant']);
      if (row.kind !== 'grant') throw invalidProtocolFrame('Invalid authorization result');
      if (row.grant === null) return { kind: 'grant', grant: null };
      const grant = requireExactRecord(row.grant, 'Authorization grant', [
        'id',
        'request',
        'revoked',
      ]);
      if (typeof grant.revoked !== 'boolean')
        throw invalidProtocolFrame('Invalid authorization revocation');
      return {
        kind: 'grant',
        grant: { id: uuid(grant.id), request: proposal(grant.request), revoked: grant.revoked },
      };
    },
  }),
} as const;

function proposal(value: unknown): AuthorizationRequest {
  const row = requireExactRecord(value, 'Authorization proposal', [
    'operationId',
    'title',
    'target',
    'capabilities',
  ]);
  const title = requireString(row.title, 'Authorization title', 256);
  if (!title.trim() || /[\u0000-\u001f\u007f]/.test(title))
    throw invalidProtocolFrame('Invalid authorization title');
  if (
    !Array.isArray(row.capabilities) ||
    !row.capabilities.length ||
    row.capabilities.length > capabilities.length
  )
    throw invalidProtocolFrame('Invalid authorization capabilities');
  const requested = row.capabilities.map((value) => {
    const capability = capabilities.find((capability) => capability === value);
    if (!capability) throw invalidProtocolFrame('Unknown authorization capability');
    return capability;
  });
  if (new Set(requested).size !== requested.length)
    throw invalidProtocolFrame('Duplicate authorization capability');
  const target = requireRecord(row.target, 'Authorization target');
  let resolved: AuthorizationRequest['target'];
  switch (target.kind) {
    case 'profile':
      requireExactRecord(target, 'Profile authorization', ['kind']);
      if (requested.some((capability) => capability !== 'notifications'))
        throw invalidProtocolFrame('Profile has no workspace authority');
      resolved = { kind: 'profile' };
      break;
    case 'session':
      requireExactRecord(target, 'Session authorization', ['kind', 'sessionId']);
      resolved = {
        kind: 'session',
        sessionId: requireId(target.sessionId, 'Authorization Session'),
      };
      break;
    case 'workspace': {
      requireExactRecord(target, 'Workspace authorization', [
        'kind',
        'workspace',
        'permissionMode',
      ]);
      const workspace = requireRecord(target.workspace, 'Authorization workspace');
      let location: Extract<AuthorizationRequest['target'], { kind: 'workspace' }>['workspace'];
      if (workspace.kind === 'project') {
        requireExactRecord(workspace, 'Authorization project', ['kind', 'projectId']);
        location = {
          kind: 'project',
          projectId: requireId(workspace.projectId, 'Authorization project'),
        };
      } else {
        requireExactRecord(workspace, 'Authorization Host path', ['kind', 'path']);
        if (workspace.kind !== 'host_path')
          throw invalidProtocolFrame('Invalid authorization workspace');
        const path = requireString(workspace.path, 'Authorization Host path', 32768);
        if (path.includes('\0')) throw invalidProtocolFrame('Invalid Host path');
        location = { kind: 'host_path', path };
      }
      const mode = target.permissionMode;
      if (mode !== 'explore' && mode !== 'ask' && mode !== 'bypass')
        throw invalidProtocolFrame('Invalid authorization permission mode');
      if (
        mode !== 'bypass' &&
        requested.some((capability) => ['write_files', 'network', 'processes'].includes(capability))
      )
        throw invalidProtocolFrame('Unattended side effects require bypass permission');
      resolved = { kind: 'workspace', workspace: location, permissionMode: mode };
      break;
    }
    default:
      throw invalidProtocolFrame('Invalid authorization target');
  }
  return { operationId: uuid(row.operationId), title, capabilities: requested, target: resolved };
}
function uuid(value: unknown): string {
  const id = requireString(value, 'Authorization identity', 36);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id))
    throw invalidProtocolFrame('Invalid authorization identity');
  return id;
}
