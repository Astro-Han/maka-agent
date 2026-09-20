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

import type { WorkHubContinuation } from '@maka/workhub/controller';
import { createElement, type ReactNode, useMemo, useState } from 'react';
import { createServicesContext } from '../../application/contracts/feature-services.js';
import type { WorkHubServices } from './ports.js';

const context = createServicesContext<{
  services: WorkHubServices;
  continuations: Map<string, WorkHubContinuation>;
}>('WorkHubServicesProvider');

/** Document-owned submission identities outlive resolver and Client remounts. */
export function WorkHubServicesProvider(props: { services: WorkHubServices; children?: ReactNode }) {
  const [continuations] = useState(() => new Map<string, WorkHubContinuation>());
  const value = useMemo(() => ({ services: props.services, continuations }), [props.services, continuations]);
  return createElement(context.Provider, { services: value }, props.children);
}
export const useWorkHubServices = () => context.useServices().services;

export function useWorkHubContinuation(hostId: string, sessionId: string | undefined) {
  const { continuations } = context.useServices();
  if (!sessionId) return undefined;
  const key = JSON.stringify([hostId, sessionId]);
  let continuation = continuations.get(key);
  if (!continuation) {
    continuation = {};
    continuations.set(key, continuation);
  }
  return continuation;
}
