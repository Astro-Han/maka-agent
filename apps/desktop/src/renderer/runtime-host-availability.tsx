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

import { useEffect, useState } from 'react';
import { Banner, Button } from '@maka/ui';
import type { DesktopRuntimeHostProfileSnapshot } from '../preload/bridge-contract.js';

export function useRuntimeHostAvailability(profileId?: string) {
  const [snapshot, setSnapshot] = useState<DesktopRuntimeHostProfileSnapshot>();
  useEffect(() => {
    let active = true;
    let revision = 0;
    const refresh = () => {
      const ticket = ++revision;
      void window.maka.runtimeHostProfiles.getSnapshot().then((value) => {
        if (active && ticket === revision) setSnapshot(value);
      }, () => { if (active && ticket === revision) setSnapshot(undefined); });
    };
    const unsubscribe = window.maka.runtimeHostProfiles.subscribeChanges(refresh);
    refresh();
    return () => { active = false; unsubscribe(); };
  }, []);
  const id = profileId ?? snapshot?.defaultProfileId ?? 'local';
  return { profileId: id, entry: snapshot?.entries.find((entry) => entry.profile.id === id) };
}

export function RuntimeHostAvailabilityNotice({
  availability, chinese, manage,
}: {
  availability: ReturnType<typeof useRuntimeHostAvailability>;
  chinese: boolean;
  manage(): void;
}) {
  const [error, setError] = useState<string>();
  const [retrying, setRetrying] = useState(false);
  if (availability.entry?.readiness === 'ready') return null;
  const connecting = availability.entry?.readiness === 'connecting' || availability.entry?.readiness === 'reconnecting';
  return <div role="status" data-host-availability="offline">
    <Banner status="warning" title={chinese
      ? connecting ? '正在连接 Host；可以继续编辑草稿，消息不会自动发送。' : 'Host 不可用；草稿已保留，可以重试或切换 Host。'
      : connecting ? 'Connecting to Host. Drafts remain editable; nothing will send automatically.' : 'Host unavailable. Drafts are retained; retry or switch Host.'} />
    {availability.entry?.message ? <p>{availability.entry.message}</p> : null}
    {error ? <p>{error}</p> : null}
    <Button variant="secondary" size="sm" label={chinese ? '重试连接' : 'Retry connection'}
      isDisabled={retrying || connecting} onClick={() => {
        setRetrying(true);
        void window.maka.runtimeHostProfiles.setEnabled(availability.profileId, true)
          .catch((cause: unknown) => setError(String(cause))).finally(() => setRetrying(false));
      }} />
    <Button variant="secondary" size="sm" label={chinese ? '切换／管理 Host' : 'Switch / manage Host'} onClick={manage} />
    <Button variant="secondary" size="sm" label={chinese ? '复制诊断' : 'Copy diagnostics'} onClick={() => {
      void window.maka.diagnostics.copyReport({ surface: 'manual', target: { profileId: availability.profileId } })
        .catch((cause: unknown) => setError(String(cause)));
    }} />
    <Button variant="secondary" size="sm" label={chinese ? '退出应用' : 'Quit application'} onClick={() => {
      void window.maka.appWindow.quit().catch((cause: unknown) => setError(String(cause)));
    }} />
  </div>;
}
