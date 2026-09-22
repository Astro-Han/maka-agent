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

import type { ReactNode } from 'react';
import type { ClientSlots } from '@maka-agent/plugin-sdk/client';
import { ClientSlot } from '@maka/ui/client-plugins';
import { Banner, SideNavItem, SideNavSection } from '@astryxdesign/core';
import { useClientHost, useClientSlots } from './context.js';
import type { ClientHostRef } from './ports.js';

/** Ephemeral selection holds the exact published registration, never a persistent route. */
export interface ClientSettingsSelection {
  readonly hostKey: string;
  readonly entry: ReturnType<typeof useClientSlots>[number];
}

export interface ClientSettingsView {
  readonly navigation: ReactNode;
  readonly page: ReactNode;
  readonly title: string;
}

export function ClientPluginSettings(props: {
  host?: ClientHostRef;
  epoch?: string;
  verified: boolean;
  locale: ClientSlots['settings.page']['locale'];
  selection?: ClientSettingsSelection;
  onSelect(selection: ClientSettingsSelection): void;
  children(view: ClientSettingsView): ReactNode;
}) {
  const title = props.locale === 'en' ? 'Extensions' : props.locale === 'zh-TW' ? '擴充功能' : '扩展';
  const unavailable = <Banner status="warning" title={props.locale === 'en'
    ? 'This extension page is unavailable. Select a page from the navigation.'
    : props.locale === 'zh-TW' ? '此擴充功能頁面已不可用，請從導覽選擇頁面。' : '此扩展页面已不可用，请从导航选择页面。'} />;
  const host = props.verified && props.epoch ? props.host : undefined;
  const { runtime, report } = useClientHost(host);
  const entries = useClientSlots(runtime).filter((entry) => entry.slot === 'settings.page' && entry.label !== undefined);
  const hostKey = JSON.stringify([host?.profileId, host?.hostId, props.epoch]);
  const selection = props.selection?.hostKey === hostKey && entries.includes(props.selection.entry) ? props.selection.entry : undefined;
  const label = (entry: typeof entries[number]) => typeof entry.label === 'string' ? entry.label : entry.label![props.locale];
  return props.children({
    title: selection ? label(selection) : title,
    navigation: entries.length ? <SideNavSection title={title}>{entries.map((entry) => (
      <SideNavItem key={JSON.stringify([entry.owner.entryId, entry.key])} label={label(entry)}
        isSelected={selection === entry} onClick={() => props.onSelect({ hostKey, entry })} />
    ))}</SideNavSection> : null,
    page: selection && runtime ? <ClientSlot store={runtime.slots} name="settings.page"
      entryId={selection.owner.entryId} entryKey={selection.key} input={{ locale: props.locale, page: selection.key }}
      onError={(identity, error) => report({ identity, error })} /> : unavailable,
  });
}
