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

import { setTimeout as delay } from 'node:timers/promises';
import { isDeepStrictEqual } from 'node:util';
import { authorizeConnectionModel } from '@maka/core/llm-connections';
import { assertOAuthStartOutput, type OAuthLoginStartInput } from '../protocol/oauth.js';
import { readRuntimeHostConnectionCatalog } from './catalog-reader.js';
import type { RuntimeHostConnection } from './connection.js';
import { abortable } from './wait-for-ready.js';

type TargetConnection = Pick<RuntimeHostConnection, 'request'>;

export interface HostedExecutionTargetInput {
  /** An explicit, caller-owned authentication attempt; never recreated on retry. */
  readonly connection?: OAuthLoginStartInput;
  readonly connectionSlug: string;
  readonly model: string;
}

export interface ConfiguredHostedExecutionTarget {
  readonly connectionId: string;
  readonly connectionSlug: string;
}

export async function configureHostedExecutionTarget(
  connection: TargetConnection,
  input: HostedExecutionTargetInput,
  signal?: AbortSignal,
): Promise<ConfiguredHostedExecutionTarget> {
  const start = input.connection;
  if (start) {
    const slug = start.target.kind === 'create' ? start.target.slug : start.target.expected.slug;
    if (slug !== input.connectionSlug) throw new Error('Authentication target does not match');
  }
  const authenticated = start ? await authenticate(connection, start, signal) : undefined;
  const before = await abortable(() => readRuntimeHostConnectionCatalog(connection), signal);
  const target = before.connections.find((candidate) => candidate.slug === input.connectionSlug);
  if (!target) throw new Error('Runtime Host connection is unavailable');
  if (
    authenticated &&
    (target.connectionId !== authenticated.connectionId ||
      !isDeepStrictEqual(target.provider, authenticated.provider))
  ) {
    throw new Error('Authenticated connection changed');
  }

  const enabledModelIds = [...new Set([...target.enabledModelIds, input.model])];
  if (!target.enabled || !target.enabledModelIds.includes(input.model)) {
    const updated = await abortable(
      () =>
        connection.request('connection.catalog.update', {
          expected: { connectionId: target.connectionId, revision: target.revision },
          changes: {
            name: target.name,
            configuration: target.configuration,
            enabled: true,
            enabledModelIds,
          },
        }),
      signal,
    );
    if (updated.kind !== 'committed') {
      throw new Error(`Runtime Host connection update was not committed: ${updated.kind}`);
    }
  }

  // Discovery contributes metadata; selecting a model does not require a listing endpoint.
  if (!target.models.some((model) => model.id === input.model)) {
    await abortable(
      () => connection.request('connection.models.fetch', { connectionId: target.connectionId }),
      signal,
    );
  }
  const after = await abortable(() => readRuntimeHostConnectionCatalog(connection), signal);
  const configured = after.connections.find(
    (candidate) => candidate.connectionId === target.connectionId,
  );
  if (
    !configured?.enabled ||
    !isDeepStrictEqual(configured.provider, target.provider) ||
    !isDeepStrictEqual(configured.configuration, target.configuration) ||
    !authorizeConnectionModel(configured, input.model)
  ) {
    throw new Error('Runtime Host did not admit the requested model target');
  }
  return { connectionId: configured.connectionId, connectionSlug: configured.slug };
}

async function authenticate(
  connection: TargetConnection,
  input: OAuthLoginStartInput,
  signal?: AbortSignal,
) {
  const deadline = AbortSignal.timeout(120_000);
  const waiting = signal ? AbortSignal.any([signal, deadline]) : deadline;
  let result = await abortable(() => connection.request('oauth.login.start', input), waiting);
  while (true) {
    assertOAuthStartOutput(input, result);
    switch (result.phase) {
      case 'authenticated':
        return result.connection;
      case 'failed':
      case 'cancelled':
        throw new Error('Runtime Host authentication did not complete');
      default:
        await delay(100, undefined, { signal: waiting });
        result = await abortable(
          () => connection.request('oauth.login.query', { attemptId: input.attemptId }),
          waiting,
        );
    }
  }
}
