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

import assert from 'node:assert/strict';
import test from 'node:test';
import { runInNewContext } from 'node:vm';
import { fileURLToPath } from 'node:url';
import * as React from 'react';
import * as JsxRuntime from 'react/jsx-runtime';
import { renderToString } from 'react-dom/server';
import { build } from 'esbuild';
import * as ClientSdk from '@maka-agent/plugin-sdk/client';
import { buildClient } from '@maka-agent/plugin-sdk/build';
import * as ClientUi from '@maka/ui/plugin';
import { LocaleProvider, ToastProvider } from '@maka/ui';

test('the bundled WorkHub surface uses the application UI and locale without a second renderer', async () => {
  let inputs;
  const source = await buildClient(
    {
      packageId: 'maka.workhub',
      entryPoint: fileURLToPath(new URL('../src/client.tsx', import.meta.url)),
    },
    async (options) => {
      const result = await build({ ...options, metafile: true });
      inputs = Object.keys(result.metafile.inputs);
      return result;
    },
  );
  assert.equal(
    inputs.some((path) => /node_modules\/(?:react-dom|@astryxdesign)\//.test(path)),
    false,
    'stateful UI and design-system implementations must come from the shared module',
  );
  let bundle;
  runInNewContext(source, {
    navigator: { platform: 'MacIntel' },
    window: {
      __MakaClientBundle__(value) {
        bundle = value;
      },
    },
  });
  const modules = {
    '@maka-agent/plugin-sdk/client': ClientSdk,
    react: React,
    'react/jsx-runtime': JsxRuntime,
    '@maka/ui/plugin': ClientUi,
  };
  const { default: plugin } = bundle.factory((name) => {
    assert.ok(Object.hasOwn(modules, name), 'unsupported shared module: ' + name);
    return modules[name];
  });
  const never = () => {
    throw new Error('Server rendering cannot invoke native operations');
  };
  const slots = new Map();
  const styles = [];
  const lifetime = new AbortController();
  await plugin.activate({
    hostEpoch: 'origin',
    signal: lifetime.signal,
    remote: { method: () => never },
    slots: { register: (name, _key, component) => slots.set(name, component) },
    style: (css) => styles.push(css),
  });
  const WorkHubRoot = slots.get('workhub.surface');
  assert.ok(WorkHubRoot, 'the actual Client entry must publish its surface');
  assert.equal(styles.length, 1);
  assert.ok(styles[0].startsWith('@layer components {'));
  assert.ok(styles[0].includes('.workHubLive'));
  const ports = new Proxy({}, { get: () => never });
  const markup = renderToString(
    React.createElement(
      LocaleProvider,
      { locale: 'zh-CN' },
      React.createElement(
        ToastProvider,
        null,
        React.createElement(WorkHubRoot, {
          sessionId: undefined,
          sessions: ports,
          native: ports,
          attachments: {
            staging: ports,
            read: never,
            prepare: never,
            formatError: never,
            copy: () => ({
              attachmentFailedTitle: '',
              tryAgain: '',
              imageAttachmentNotDirectTitle: '',
              imageAttachmentNotDirectDescription: '',
            }),
          },
          contextUsage: ports,
        }),
      ),
    ),
  );
  assert.ok(
    markup.includes('aria-label="工作台"'),
    'the Client must consume the application’s locale context',
  );
  assert.ok(markup.includes('问个问题、管理任务，或让我帮你操作 Maka。'));
});
