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


import { z } from 'zod';
import {
  nativeRuntimeHostDeploymentSchema as deployment,
  nativeRuntimeHostIdentitySchema as host,
  type NativeRuntimeHostDeploymentStatus,
} from './native-runtime-host-deployment.js';

const expected = deployment.pick({ deploymentId: true, configRevision: true });
const policy = z.enum(['manual', 'rust_preview']);
export const nativeRuntimeHostUpdatePolicySchema = z.object({
  revision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  policy,
  nextCheckMs: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
  lastError: z.string().max(8192).nullable(),
}).strict();
const settings = deployment.pick({
  mode: true,
  websocket: true,
  projectDirectoryRoots: true,
}).partial().extend({
  // Absent preserves the current policy; null restores the Host default.
  projectDirectoryRoots: deployment.shape.projectDirectoryRoots.nullable(),
}).strict();

export const nativeRuntimeHostManagementRequestSchema = z.discriminatedUnion('action', [
  z.object({ action: z.literal('status') }).strict(),
  z.object({ action: z.literal('logs') }).strict(),
  z.object({ action: z.literal('start') }).strict(),
  z.object({ action: z.literal('install'), settings }).strict(),
  z.object({ action: z.literal('stop'), expected }).strict(),
  z.object({ action: z.literal('restart'), expected }).strict(),
  z.object({ action: z.literal('uninstall'), expected }).strict(),
  z.object({ action: z.literal('update'), expected, settings }).strict(),
  z.object({ action: z.literal('upgrade'), expected }).strict(),
  z.object({ action: z.literal('update_policy') }).strict(),
  z.object({ action: z.literal('set_update_policy'), expected,
    expectedPolicyRevision: nativeRuntimeHostUpdatePolicySchema.shape.revision, policy }).strict(),
  z.object({ action: z.literal('reconcile'), expected }).strict(),
]);

export const nativeRuntimeHostMutationSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('active_tasks'), deployment, target: deployment.optional() }).strict(),
  z.object({ kind: z.literal('stopped'), deployment }).strict(),
  z.object({ kind: z.literal('ready'), deployment, host }).strict(),
  z.object({
    kind: z.literal('unregistered'),
    deployment,
    cleanup: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('complete') }).strict(),
      z.object({ kind: z.literal('pending'), message: z.string().max(8192) }).strict(),
    ]),
  }).strict(),
]);

export const nativeRuntimeHostLogsSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('not_captured') }).strict(),
  z.object({
    kind: z.literal('tail'),
    source: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('stderr') }).strict(),
      z.object({ kind: z.literal('journal'), entryLimit: z.literal(200) }).strict(),
    ]),
    text: z.string().max(49_152),
    byteTruncated: z.boolean(),
  }).strict(),
]);

export type NativeRuntimeHostManagementRequest = z.infer<typeof nativeRuntimeHostManagementRequestSchema>;
export type NativeRuntimeHostMutation = z.infer<typeof nativeRuntimeHostMutationSchema>;
export type NativeRuntimeHostLogs = z.infer<typeof nativeRuntimeHostLogsSchema>;
export type NativeRuntimeHostExpected = z.infer<typeof expected>;
export type NativeRuntimeHostSettings = z.infer<typeof settings>;

export interface NativeRuntimeHostManagementResult {
  readonly status: NativeRuntimeHostDeploymentStatus;
  readonly outcome?: NativeRuntimeHostMutation | { readonly kind: 'active_tasks' };
  readonly logs?: NativeRuntimeHostLogs;
  readonly updatePolicy?: z.infer<typeof nativeRuntimeHostUpdatePolicySchema>;
  readonly schedulingError?: string;
}
