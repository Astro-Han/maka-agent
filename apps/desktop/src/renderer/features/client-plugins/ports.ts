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

import type { ClientDescriptor } from '@maka-agent/plugin-sdk/client';
import type { ClientSnapshot, ClientRemoteFactory } from '@maka/ui/client-plugins';

export interface ClientHostRef { readonly profileId: string; readonly hostId: string }
export interface ClientPluginServices {
  connect(host: ClientHostRef): {
    readonly remote: ClientRemoteFactory;
    session(sessionId: string): Promise<string>;
    snapshot(signal: AbortSignal): Promise<ClientSnapshot>;
    source(descriptor: ClientDescriptor, signal: AbortSignal): Promise<string>;
    subscribe(listener: () => void): () => void;
  };
}
