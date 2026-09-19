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

const count = z.number().int().nonnegative().safe();
const identity = z.string().min(1).max(128);
const digest = z.string().regex(/^[a-f0-9]{64}$/u);
const path = z.string().min(1).max(32_768);
const message = z.string().max(8192);
export const nativeRuntimeHostDeploymentSchema = z.object({
  deploymentId: z.string().uuid(),
  configRevision: count.positive(),
  rootId: digest,
  rootPath: path,
  executable: path,
  sha256: digest,
  mode: z.enum(['on_demand', 'supervised']),
  websocket: z.string().regex(/^127\.0\.0\.1:(?:0|[1-9][0-9]{0,4})$/u)
    .refine((value) => Number(value.split(':')[1]) <= 65_535),
  projectDirectoryRoots: z.array(z.object({
    label: z.string().min(1).max(256),
    path,
  }).strict()).max(8).optional(),
  admission: z.literal('revoked').optional(),
}).strict();
const deployment = nativeRuntimeHostDeploymentSchema;
export const nativeRuntimeHostIdentitySchema = z.object({
  hostEpoch: identity,
  pid: count.positive(),
  port: count.min(1).max(65_535),
}).strict();

const unavailable = z.object({ kind: z.literal('unavailable'), message }).strict();
const status = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('not_installed') }).strict(),
  z.object({ kind: z.literal('incomplete') }).strict(),
  z.object({
    kind: z.literal('installed'),
    deployment,
    pendingUpdate: deployment.nullable(),
    operation: z.enum(['idle', 'in_progress', 'unknown']).optional(),
    supervisor: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('on_demand') }).strict(),
      z.object({ kind: z.literal('missing') }).strict(),
      unavailable,
      z.object({
        kind: z.literal('present'),
        state: z.enum(['stopped', 'starting', 'running', 'stopping', 'failed']),
        enabled: z.boolean().nullable(),
        pid: count.positive().nullable(),
        lastResult: z.number().int().safe().nullable(),
      }).strict(),
    ]),
    host: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('not_admitted') }).strict(),
      unavailable,
      z.object({
        kind: z.literal('connected'),
        identity: nativeRuntimeHostIdentitySchema,
        activity: z.object({
          state: z.enum(['starting', 'containing', 'recovering', 'ready', 'draining']),
          connections: count,
          activeOperations: count,
          activeResidencies: count,
        }).strict(),
      }).strict(),
    ]),
  }).strict(),
]);

export type NativeRuntimeHostDeployment = z.infer<typeof deployment>;
export type NativeRuntimeHostDeploymentStatus = z.infer<typeof status>;

export function decodeNativeRuntimeHostDeploymentStatus(
  value: unknown,
  rootId: string,
): NativeRuntimeHostDeploymentStatus {
  const result = status.parse(value);
  if (result.kind === 'installed') {
    if (result.deployment.rootId !== rootId) {
      throw new Error('Native Host deployment belongs to another Root');
    }
    const pending = result.pendingUpdate;
    if (pending && (
      pending.rootId !== rootId ||
      pending.rootPath !== result.deployment.rootPath ||
      pending.deploymentId !== result.deployment.deploymentId ||
      pending.configRevision !== result.deployment.configRevision + 1 ||
      pending.admission !== undefined ||
      result.deployment.admission !== undefined
    )) {
      throw new Error('Native Host pending update belongs to another deployment revision');
    }
  }
  return result;
}
