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
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { LocaleProvider } from '@maka/ui';
import type * as HostModule from '../../renderer/features/module-hub/index.js';
import { build } from 'esbuild';
import { mkdtemp, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createFakeModuleHubHostModel } from '../../renderer/features/module-hub/testing.js';

test('extension route mounts supplied Client content without owning its business controller', async (t) => {
  const repo = resolve(import.meta.dirname, '../../../../..');
  const directory = await mkdtemp(resolve(import.meta.dirname, 'module-hub-render-'));
  t.after(() => rm(directory, {recursive:true,force:true}));
  const outfile = resolve(directory, 'host.mjs');
  await build({entryPoints:[resolve(repo, 'apps/desktop/src/renderer/features/module-hub/ui/module-hub-host.tsx')],
    outfile,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',logLevel:'silent'});
  const {ModuleHubHostView} = await import(pathToFileURL(outfile).href) as typeof HostModule;
  const extensionContent = createElement('div', {'data-client-domain':true}, 'plugin owned');
  const render = (model: ReturnType<typeof createFakeModuleHubHostModel>) =>
    renderToStaticMarkup(createElement(LocaleProvider, {locale:'en',children:
      createElement(ModuleHubHostView, {model, extensionContent}),
    }));
  const skills = render(createFakeModuleHubHostModel({section:'extensions',module:'skills'}));
  assert.match(skills, /data-client-domain="true"/);
  assert.match(skills, /plugin owned/);
  assert.equal(render(createFakeModuleHubHostModel({section:'sessions'})), '');
});
