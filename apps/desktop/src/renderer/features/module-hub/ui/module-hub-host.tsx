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

import {
  DailyReviewPage,
  ModuleHubSelector,
  getSharedUiCopy,
  useUiLocale,
  useToast,
  type ModuleHubHeader,
} from '@maka/ui';
import { useState, type ReactNode } from 'react';
import { McpPage } from '../../../mcp-page.js';
import type { ModuleHubHostModel } from '../controller/use-module-hub-controller.js';
import { resolveModuleHubHostRoute } from '../controller/module-hub-route.js';
import { useModuleHubHostModel } from './module-hub-provider.js';

/** Selects and mounts exactly one Module Hub leaf for the Shell selection. */
interface Content {
  extensionContent?: ReactNode;
  applicationContent?: (section: string, action?: ModuleHubHostModel['action']) => ReactNode;
}
export function ModuleHubHost(props: Content) {
  return <ModuleHubHostView model={useModuleHubHostModel()} {...props} />;
}

/** Environment-free view seam for focused tests and Storybook. */
export function ModuleHubHostView({ model, extensionContent, applicationContent }: Content & { model: ModuleHubHostModel }) {
  const copy = getSharedUiCopy(useUiLocale()).moduleHubs;
  const selection = model.selection;
  const route = resolveModuleHubHostRoute(selection);

  if (route === 'skills' || route === 'mcp') {
    const header: ModuleHubHeader = {
      title: copy.extensions.title,
      subtitle: copy.extensions.description,
      badge: (
        <ModuleHubSelector
          hub="extensions"
          value={route}
          onChange={(module) =>
            model.selectModule({ section: 'extensions', module })
          }
        />
      ),
    };
    if (route === 'mcp') {
      // Explicit leaf-owner exception: MCP keeps its existing page-owned
      // controller and direct bridge; Module Hub only selects and mounts it.
      return <McpPage hubHeader={header} />;
    }
    return (
      <section className="maka-main detailPane maka-module-main agents-chat-panel" data-page-shell="layout" data-module={route} aria-label={header.title}>
        <header><h1>{header.title}</h1>{header.badge}</header>
        {extensionContent}
      </section>
    );
  }

  if (route === 'scheduled-tasks' || route === 'daily-review') {
    const header: ModuleHubHeader = {
      title: copy.automations.title,
      subtitle: copy.automations.description,
      badge: (
        <ModuleHubSelector
          hub="automations"
          value={route}
          onChange={(module) =>
            model.selectModule({ section: 'automations', module })
          }
        />
      ),
    };
    if (route === 'scheduled-tasks') {
      return (
        <section className="maka-main detailPane maka-module-main agents-chat-panel" data-page-shell="layout" data-module={route} aria-label={header.title}>
          <header><h1>{header.title}</h1>{header.badge}</header>
          <PowerSetting value={model.keepSystemAwake} />
          {applicationContent?.(route, model.action)}
        </section>
      );
    }
    const dailyReview = model.dailyReview;
    return (
      <DailyReviewPage
        hubHeader={header}
        bridge={dailyReview.bridge}
        onSelectSession={model.openSession}
        onCopyMarkdown={dailyReview.copyMarkdown}
        onAppendMarkdown={dailyReview.appendMarkdown}
        onSaveMarkdown={dailyReview.saveMarkdown}
      />
    );
  }

  return null;
}

/** Desktop power policy belongs to the application, not to any scheduler. */
function PowerSetting({ value }: { value: ModuleHubHostModel['keepSystemAwake'] }) {
  const locale = useUiLocale();
  const toast = useToast();
  const [pending, setPending] = useState(false);
  if (!value.supported) return null;
  return <label>
    <input type="checkbox" checked={value.keepSystemAwake ?? false}
      disabled={pending || value.keepSystemAwake === undefined}
      onChange={(event) => {
        setPending(true);
        void value.setKeepSystemAwake(event.target.checked)
          .catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))
          .finally(() => setPending(false));
      }} />
    {locale === 'en' ? 'Keep this computer awake' : '保持此电脑唤醒'}
  </label>;
}
