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
import type { DesktopCapabilityService } from './runtime-host-native-capabilities.js';

const payload = z.object({
  packageId: z.string().min(1).max(256),
  notification: z.object({
    id: z.string().min(1).max(256),
    title: z.string().trim().min(1).max(512),
    body: z.string().max(32 * 1024),
    destination: z.discriminatedUnion('kind', [
      z.object({ kind: z.literal('local') }).strict(),
      z.object({
        kind: z.literal('channel'),
        channel: z.string().min(1).max(256),
        recipient: z.string().min(1).max(512),
      }).strict(),
    ]),
  }).strict(),
}).strict();

export function pluginNotifications(effects: {
  local(packageId: string, title: string, body: string): void | Promise<void>;
  channel(channel: string, recipient: string, title: string, body: string): Promise<void>;
}): DesktopCapabilityService {
  return {
    serviceId: 'maka_notifications',
    version: '1',
    async call(method, input) {
      if (method !== 'send') throw new Error('Unknown notification operation');
      const { packageId, notification: { title, body, destination } } = payload.parse(input);
      if (destination.kind === 'local') await effects.local(packageId, title, body);
      else await effects.channel(destination.channel, destination.recipient, title, body);
      return { ok: true };
    },
  };
}
