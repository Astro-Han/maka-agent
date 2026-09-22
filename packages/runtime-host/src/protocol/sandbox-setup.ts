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

import { requireExactRecord, requireString } from './codec.js';
import { invalidProtocolFrame } from './errors.js';
import { defineOperation } from './operation-spec.js';

export type SandboxSetupStatus =
  | 'not_required'
  | 'not_configured'
  | 'setup_required'
  | 'ready'
  | 'removing'
  | 'busy';

const ERRORS = [
  'host_not_ready',
  'host_draining',
  'unauthorized',
  'user_cancelled',
  'operation_unavailable',
  'invalid_request',
  'operation_conflict',
  'outcome_unknown',
  'internal_failure',
] as const;

function input(value: unknown): Record<string, never> {
  requireExactRecord(value, 'sandbox setup input', []);
  return {};
}
function output(value: unknown): SandboxSetupStatus {
  const status = requireString(value, 'sandbox setup status', 32);
  switch (status) {
    case 'not_required':
    case 'not_configured':
    case 'setup_required':
    case 'ready':
    case 'removing':
    case 'busy':
      return status;
    default:
      throw invalidProtocolFrame('Invalid sandbox setup status');
  }
}
function spec(mode: 'query' | 'command') {
  return defineOperation<Record<string, never>, SandboxSetupStatus, (typeof ERRORS)[number]>({
    mode,
    availability: 'ready',
    errors: ERRORS,
    decodeInput: input,
    decodeOutput: output,
  });
}
export const SANDBOX_SETUP_OPERATION_SPECS = {
  'sandbox.setup.query': spec('query'),
  'sandbox.setup.install': spec('command'),
  'sandbox.setup.remove': spec('command'),
} as const;
