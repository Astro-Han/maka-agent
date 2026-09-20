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

import { createPluginRemote } from '@maka/runtime-host/client/plugin-remote';
import type { ClientRemoteFactory } from '@maka/ui/client-plugins';
import type { MakaBridge } from '../../../preload/bridge-contract.js';
import type { ClientHostRef } from '../../features/client-plugins/index.js';

export function clientPluginRemote(
  transport: MakaBridge['clientPlugins']['remote'],
  host: ClientHostRef,
  connectionEpoch: string,
): ClientRemoteFactory {
  return (identity, signal) => createPluginRemote(
    (input) => transport(host, connectionEpoch, input), identity, signal,
  );
}
