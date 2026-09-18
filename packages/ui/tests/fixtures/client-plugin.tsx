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

import { useState } from 'react';
import type { ClientPlugin } from '@maka-agent/plugin-sdk/client';

const plugin: ClientPlugin = {
  activate(ctx, config) {
    const label = typeof config === 'object' && config !== null && 'label' in config
      ? String(config.label) : 'Plugin';
    ctx.style('[data-plugin-example] { padding: 12px; border: 1px solid currentColor; }');
    ctx.slots.register('session.composer.before', 'example', function Example() {
      const [count, setCount] = useState(0);
      return <button type="button" data-plugin-example onClick={() => {
        ctx.signal.throwIfAborted();
        setCount((value) => value + 1);
      }}>{label}: {count}</button>;
    });
  },
};
export default plugin;
