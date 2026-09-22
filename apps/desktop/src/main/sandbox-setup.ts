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

import type { SandboxSetupStatus } from '@maka/runtime-host/protocol';
import { RuntimeHostOperationError } from '@maka/runtime-host/client';
import type { MessageBoxOptions } from 'electron';

export function sandboxSetupDialog(locale: string): MessageBoxOptions {
  const zh = locale.startsWith('zh');
  const traditional = locale === 'zh-TW';
  return {
    type: 'question',
    title: zh ? 'Windows 沙箱' : 'Windows sandbox',
    message: zh ? (traditional ? '啟用沙箱以繼續？' : '启用沙箱以继续？') : 'Enable the sandbox to continue?',
    detail: zh
      ? (traditional ? 'Windows 會要求管理員確認，Maka 將自動完成設定。取消會保留草稿，不會傳送訊息。' : 'Windows 会请求管理员确认，Maka 将自动完成配置。取消会保留草稿，不会发送消息。')
      : 'Windows will ask for administrator approval. Maka handles setup automatically. Cancel keeps your draft without sending it.',
    buttons: zh ? ['取消', traditional ? '啟用沙箱' : '启用沙箱'] : ['Cancel', 'Enable sandbox'],
    cancelId: 0,
    defaultId: 1,
  };
}

export function sandboxSetupUnavailable(locale: string, status: SandboxSetupStatus): Error {
  const zh = locale.startsWith('zh');
  const traditional = locale === 'zh-TW';
  if (status === 'busy') return new Error(zh
    ? (traditional ? '沙箱正在初始化，請稍後重試。草稿已保留。' : '沙箱正在初始化，请稍后重试。草稿已保留。')
    : 'Sandbox setup is in progress. Your draft is preserved; try again shortly.');
  return new Error(zh
    ? (traditional ? '沙箱尚未就緒，請重試啟用。草稿已保留。' : '沙箱尚未就绪，请重试启用。草稿已保留。')
    : 'The sandbox is not ready. Retry setup; your draft is preserved.');
}

/** One client generation owns consent and setup; installation facts stay in Host. */
export function createSandboxSetup(ports: {
  query(): Promise<SandboxSetupStatus>;
  install(): Promise<SandboxSetupStatus>;
  confirm(): Promise<boolean>;
  isCurrent(): boolean;
  unavailable(status: SandboxSetupStatus): Error;
}): () => Promise<boolean> {
  let pending: Promise<boolean> | undefined;
  async function settled(status: SandboxSetupStatus): Promise<SandboxSetupStatus> {
    const deadline = performance.now() + 180_000;
    // Another client (or an accepted helper whose reply was lost) may own
    // setup. Observe its durable outcome instead of asking for another UAC.
    while (status === 'busy' && ports.isCurrent()) {
      const remaining = deadline - performance.now();
      if (remaining <= 0) break;
      await new Promise<void>((resolve) => setTimeout(resolve, Math.min(500, remaining)));
      if (!ports.isCurrent()) break;
      status = await ports.query();
    }
    return status;
  }
  async function prepare(): Promise<boolean> {
    if (!ports.isCurrent()) return false;
    const status = await settled(await ports.query());
    if (!ports.isCurrent()) return false;
    if (status === 'ready' || status === 'not_required') return true;
    if (status === 'busy') throw ports.unavailable(status);
    if (!(await ports.confirm()) || !ports.isCurrent()) return false;
    let installed: SandboxSetupStatus;
    try {
      installed = await settled(await ports.install());
    } catch (error) {
      if (error instanceof RuntimeHostOperationError && error.code === 'user_cancelled') return false;
      // An accepted helper may finish even when its reply is lost. Never
      // replay installation on this path or submit against an unknown result.
      if (!ports.isCurrent()) return false;
      installed = await settled(await ports.query()).catch(() => { throw error; });
      if (!ports.isCurrent()) return false;
      if (installed !== 'ready') throw error;
    }
    if (!ports.isCurrent()) return false;
    if (installed !== 'ready') throw ports.unavailable(installed);
    return true;
  }
  return () => pending ??= prepare().finally(() => { pending = undefined; });
}
