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

import type { SandboxMode } from './permission.js';

/** User-selected isolation and approval policy, committed together. */
export interface ExecutionPolicy {
  readonly sandboxMode: SandboxMode;
  readonly approvalPolicy: ApprovalPolicy;
}

export function approvalPoliciesEqual(left: ApprovalPolicy, right: ApprovalPolicy): boolean {
  if (left.kind !== right.kind) return false;
  return (
    left.kind !== 'granular' ||
    (right.kind === 'granular' &&
      left.sandbox === right.sandbox &&
      left.rules === right.rules &&
      left.permissions === right.permissions &&
      left.client === right.client)
  );
}

export function executionPoliciesEqual(left: ExecutionPolicy, right: ExecutionPolicy): boolean {
  return (
    left.sandboxMode === right.sandboxMode &&
    approvalPoliciesEqual(left.approvalPolicy, right.approvalPolicy)
  );
}

/** Whether the Host may ask, independent of the sandbox's existing access. */
export type ApprovalPolicy =
  | { readonly kind: 'on-request' | 'never' }
  | {
      readonly kind: 'granular';
      readonly sandbox: boolean;
      readonly rules: boolean;
      readonly permissions: boolean;
      readonly client: boolean;
    };

export type NetworkAccess =
  | 'denied'
  | 'allowed'
  | {
      readonly restricted: {
        readonly destinations: readonly { readonly host: string; readonly port: number }[];
      };
    };

export interface AdditionalAccess {
  readonly filesystem: readonly {
    readonly path: string;
    readonly scope: 'subtree' | 'exact';
    readonly access: 'read' | 'write';
  }[];
  readonly network: NetworkAccess;
}

export type PermissionDecision =
  | { readonly decision: 'deny' }
  | {
      readonly decision: 'allow';
      readonly permissions: AdditionalAccess;
      readonly scope: 'once' | 'turn' | 'session';
    };

export interface AccessRequest {
  readonly reason: string;
  readonly command: { readonly command: string; readonly cwd: string } | null;
  readonly permissions: AdditionalAccess;
}

export interface PermissionsResponse {
  readonly requestId: string;
  readonly decision: PermissionDecision;
}

export function decodePermissionsResponse(value: unknown): PermissionsResponse {
  const response = object(value);
  keys(response, ['requestId', 'decision']);
  return {
    requestId: text(response.requestId, 256),
    decision: decodePermissionDecision(response.decision),
  };
}

export function decodeApprovalPolicy(value: unknown): ApprovalPolicy {
  const policy = object(value);
  if (policy.kind === 'on-request' || policy.kind === 'never') {
    keys(policy, ['kind']);
    return { kind: policy.kind };
  }
  keys(policy, ['kind', 'sandbox', 'rules', 'permissions', 'client']);
  if (
    policy.kind !== 'granular' ||
    typeof policy.sandbox !== 'boolean' ||
    typeof policy.rules !== 'boolean' ||
    typeof policy.permissions !== 'boolean' ||
    typeof policy.client !== 'boolean'
  )
    throw new Error('Invalid approval policy');
  return {
    kind: 'granular',
    sandbox: policy.sandbox,
    rules: policy.rules,
    permissions: policy.permissions,
    client: policy.client,
  };
}

/** Paths retain executor-native spelling. Only the execution Host decides
 * containment, resolves paths, and validates a partial grant against a request. */
export function decodeAdditionalAccess(value: unknown): AdditionalAccess {
  const access = object(value);
  keys(access, ['filesystem', 'network']);
  if (!Array.isArray(access.filesystem) || access.filesystem.length > 32) {
    throw new Error('Invalid additional access');
  }
  return {
    network: decodeNetworkAccess(access.network),
    filesystem: access.filesystem.map((value) => {
      const rule = object(value);
      keys(rule, ['path', 'scope', 'access']);
      if (
        (rule.scope !== 'subtree' && rule.scope !== 'exact') ||
        (rule.access !== 'read' && rule.access !== 'write')
      )
        throw new Error('Invalid access rule');
      return { path: text(rule.path, 4096), scope: rule.scope, access: rule.access };
    }),
  };
}

function decodeNetworkAccess(value: unknown): NetworkAccess {
  if (value === 'denied' || value === 'allowed') return value;
  const network = object(value);
  keys(network, ['restricted']);
  const restricted = object(network.restricted);
  keys(restricted, ['destinations']);
  if (!Array.isArray(restricted.destinations) || restricted.destinations.length > 128)
    throw new Error('Invalid network destinations');
  return {
    restricted: {
      destinations: restricted.destinations.map((value) => {
        const destination = object(value);
        keys(destination, ['host', 'port']);
        if (
          typeof destination.port !== 'number' ||
          !Number.isInteger(destination.port) ||
          destination.port < 1 ||
          destination.port > 65535
        )
          throw new Error('Invalid network destination port');
        return { host: text(destination.host, 253), port: destination.port };
      }),
    },
  };
}

export function decodePermissionDecision(value: unknown): PermissionDecision {
  const decision = object(value);
  if (decision.decision === 'deny') {
    keys(decision, ['decision']);
    return { decision: 'deny' };
  }
  keys(decision, ['decision', 'permissions', 'scope']);
  if (
    decision.decision !== 'allow' ||
    (decision.scope !== 'once' && decision.scope !== 'turn' && decision.scope !== 'session')
  ) {
    throw new Error('Invalid permission decision');
  }
  return {
    decision: 'allow',
    scope: decision.scope,
    permissions: decodeAdditionalAccess(decision.permissions),
  };
}

export function decodeAccessRequest(value: unknown): AccessRequest {
  const request = object(value);
  keys(request, ['reason', 'command', 'permissions']);
  let command: AccessRequest['command'] = null;
  if (request.command !== null) {
    const input = object(request.command);
    keys(input, ['command', 'cwd']);
    command = { command: text(input.command, 8192), cwd: text(input.cwd, 4096) };
  }
  return {
    reason: text(request.reason, 4096),
    command,
    permissions: decodeAdditionalAccess(request.permissions),
  };
}

export function permissionDecisionsEqual(
  left: PermissionDecision,
  right: PermissionDecision,
): boolean {
  return (
    JSON.stringify(decodePermissionDecision(left)) ===
    JSON.stringify(decodePermissionDecision(right))
  );
}

function object(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Expected permissions object');
  }
  return value as Record<string, unknown>;
}

function keys(value: Record<string, unknown>, expected: readonly string[]): void {
  if (
    Object.keys(value).length !== expected.length ||
    expected.some((key) => !Object.hasOwn(value, key))
  ) {
    throw new Error('Unexpected permissions fields');
  }
}

function text(value: unknown, max: number): string {
  if (
    typeof value !== 'string' ||
    !value.trim() ||
    value.includes('\0') ||
    new TextEncoder().encode(value).length > max
  )
    throw new Error('Invalid permissions text');
  return value;
}
