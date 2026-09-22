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

import { PermissionsPrompt } from '@maka/ui';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';

const meta = {
  title: 'Product/Permissions',
  component: PermissionsPrompt,
  parameters: {layout: 'fullscreen'},
  decorators: [(Story) => (
    // ChatComposerRegion's owning layout is inline in the application shell.
    // No native titlebar/sidebar is reproduced; this is its main-column slot.
    <div className="maka-panel maka-panel-detail" style={{minHeight: '100dvh', display: 'flex', flexDirection: 'column'}}>
      <div className="maka-detail-with-artifacts">
        <div className="mainColumn" style={{justifyContent: 'flex-end'}}><Story /></div>
      </div>
    </div>
  )],
} satisfies Meta<typeof PermissionsPrompt>;
export default meta;
type Story = StoryObj<typeof meta>;

// Real path: managed Rust Session → Shell requests additional permissions →
// canonical pending interaction replaces the main conversation's composer.
export const Command: Story = {
  args: {
    request: {
      type: 'permissions_request', id: 'permissions-event', requestId: 'permissions-request',
      turnId: 'turn', toolUseId: 'command', ts: 1,
      request: {
        reason: 'Download build dependencies and write the release outside the workspace.',
        command: {command: 'npm ci && npm run build -- --outDir /Users/maka/releases/current', cwd: '/Users/maka/projects/application'},
        permissions: {
          filesystem: [
            {path: '/Users/maka/releases/current', access: 'write', scope: 'subtree'},
            {path: '/Users/maka/.config/signing.json', access: 'read', scope: 'exact'},
          ],
          network: 'allowed',
        },
      },
    },
    onRespond: fn(),
  },
};
