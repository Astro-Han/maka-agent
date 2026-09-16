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


import { useEffect, useRef, useState } from 'react';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { Layout, LayoutContent } from '@astryxdesign/core/Layout';
import { Banner, Button, Selector, TextInput, useUiLocale } from '@maka/ui';
import type {
  NativeRuntimeHostExpected,
  NativeRuntimeHostManagementRequest,
  NativeRuntimeHostManagementResult,
  NativeRuntimeHostSettings,
} from '../../shared/native-runtime-host-management.js';
import { getSettingsProjectsCopy } from '../locales/settings-projects-copy.js';
import { RuntimeHostProjectDirectoryEditor } from './runtime-host-project-directory-editor.js';

export function NativeRuntimeHostManagementDialog(props: { readonly onClose: () => void }) {
  const locale = useUiLocale();
  const zh = locale === 'zh-CN';
  const copy = {
    title: zh ? '本机 Host' : 'Local Host',
    refresh: zh ? '刷新' : 'Refresh',
    install: zh ? '安装并启动' : 'Install and start',
    start: zh ? '启动' : 'Start',
    stop: zh ? '停止' : 'Stop',
    restart: zh ? '重启' : 'Restart',
    uninstall: zh ? '卸载（保留数据）' : 'Uninstall (retain data)',
    confirm: zh ? '确认卸载' : 'Confirm uninstall',
    cancel: zh ? '取消' : 'Cancel',
    update: zh ? '应用此 Desktop 的 Host 版本' : 'Apply this Desktop’s Host version',
    edit: zh ? '编辑配置' : 'Edit configuration',
    save: zh ? '应用配置与当前版本' : 'Apply settings and current version',
    reconcile: zh ? '继续待处理更新' : 'Continue pending update',
    logs: zh ? '日志' : 'Logs',
    mode: zh ? '运行方式' : 'Launch policy',
    onDemand: zh ? '按需启动' : 'On demand',
    supervised: zh ? '后台常驻' : 'Account service',
    address: zh ? '本机监听地址' : 'Loopback listener',
    directories: zh ? '项目目录' : 'Project directories',
    default: zh ? '使用 Host 默认目录' : 'Use Host default',
    custom: zh ? '自定义（空列表表示不公开目录）' : 'Custom (empty exposes no directories)',
    conflict: zh ? '配置已改变；取消编辑并刷新后重试。' : 'Configuration changed; cancel editing and refresh.',
    busy: zh ? 'Host 仍有活动任务，尚未完成交接。' : 'Host still owns active work; handoff is not complete.',
    pending: zh ? '目标已持久化，尚未激活。' : 'Target is durable but not yet active.',
    unknown: zh ? '操作结果未确认。请刷新状态后再决定下一步。' : 'Outcome is unconfirmed. Refresh status before continuing.',
    notInstalled: zh ? '未安装托管部署；当前会话不受影响。' : 'No managed deployment; current sessions are unaffected.',
    incomplete: zh ? '安装记录不完整，需要修复后才能继续。' : 'Installation is incomplete and requires repair.',
    stopped: zh ? '未连接' : 'Disconnected',
    ready: zh ? '已连接' : 'Connected',
    revoked: zh ? '已卸载；数据保留' : 'Uninstalled; data retained',
    cleanupPending: zh ? '部署已撤销，但后台服务尚未清理完毕；可重试卸载。' : 'Deployment is revoked, but service cleanup is incomplete; retry uninstall.',
    notCaptured: zh ? '此运行方式没有捕获服务日志。' : 'Service logs are not captured in this mode.',
    truncated: zh ? '仅显示日志尾部，前面的内容已省略。' : 'Log tail only; earlier output is omitted.',
  };
  const [result, setResult] = useState<NativeRuntimeHostManagementResult>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [confirmUninstall, setConfirmUninstall] = useState<NativeRuntimeHostExpected>();
  const [edit, setEdit] = useState<{
    expected: NativeRuntimeHostExpected;
    settings: NativeRuntimeHostSettings;
  }>();
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    void window.maka.runtimeHostManagement.runNative({ action: 'status' }).then((value) => {
      if (alive.current && value) setResult(value);
    }, (cause: unknown) => { if (alive.current) setError(String(cause)); });
    return () => { alive.current = false; };
  }, []);

  async function run(request: NativeRuntimeHostManagementRequest) {
    setBusy(true);
    setError(undefined);
    try {
      const value = await window.maka.runtimeHostManagement.runNative(request);
      if (!alive.current) return;
      if (!value) throw new Error('Native Host management is unavailable');
      setResult(value);
      if (request.action !== 'status' && request.action !== 'logs' &&
        value.outcome?.kind !== 'active_tasks') {
        setEdit(undefined);
        setConfirmUninstall(undefined);
      }
    } catch (cause) {
      if (alive.current) setError(`${copy.unknown} ${String(cause)}`);
    } finally {
      if (alive.current) setBusy(false);
    }
  }
  const status = result?.status;
  const deployment = status?.kind === 'installed' ? status.deployment : undefined;
  const active = deployment?.admission === undefined && deployment !== undefined;
  const expected = deployment && {
    deploymentId: deployment.deploymentId, configRevision: deployment.configRevision,
  };
  const conflict = edit && (!expected || edit.expected.deploymentId !== expected.deploymentId ||
    edit.expected.configRevision !== expected.configRevision);
  const button = (label: string, request: NativeRuntimeHostManagementRequest) => (
    <Button variant="secondary" size="sm" label={label} isDisabled={busy} onClick={() => void run(request)} />
  );
  return (
    <Dialog isOpen onOpenChange={(open) => { if (!open && !busy) props.onClose(); }}
      purpose="form" width={640} maxHeight="calc(100dvh - 64px)">
      <Layout header={<DialogHeader title={copy.title}
        onOpenChange={(open) => { if (!open && !busy) props.onClose(); }} />}
        content={<LayoutContent padding={4}>
          <div className="settingsRuntimeHostManagement">
            {error ? <Banner status="error" title={error} /> : null}
            {result?.outcome?.kind === 'active_tasks' ? <Banner status="warning" title={copy.busy} /> : null}
            {status?.kind === 'not_installed' ? <p>{copy.notInstalled}</p> : null}
            {status?.kind === 'incomplete' ? <Banner status="error" title={copy.incomplete} /> : null}
            {status?.kind === 'installed' ? <>
              <p>{deployment?.admission === 'revoked' ? copy.revoked :
                status.host.kind === 'connected' ? copy.ready : copy.stopped}
                {' · '}{status.deployment.mode === 'supervised' ? copy.supervised : copy.onDemand}
                {' · r'}{status.deployment.configRevision}</p>
              <p><code>{status.deployment.rootPath}</code></p>
              {status.host.kind === 'unavailable' ? <p>{status.host.message}</p> : null}
              {status.supervisor.kind === 'unavailable' ? <Banner status="warning" title={status.supervisor.message} /> : null}
              {status.deployment.admission === 'revoked' &&
                (status.supervisor.kind === 'present' || status.supervisor.kind === 'unavailable')
                ? <Banner status="warning" title={copy.cleanupPending} /> : null}
              {status.pendingUpdate ? <Banner status="warning"
                title={`${copy.pending} r${status.pendingUpdate.configRevision}`} /> : null}
              {result?.outcome?.kind === 'unregistered' && result.outcome.cleanup.kind === 'pending'
                ? <Banner status="warning" title={result.outcome.cleanup.message} /> : null}
            </> : null}
            <div className="settingsRuntimeHostManagementActions">
              {button(copy.refresh, { action: 'status' })}
              {status?.kind === 'not_installed' || deployment?.admission === 'revoked'
                ? button(copy.install, { action: 'install', settings: { mode: 'on_demand' } }) : null}
              {active && expected ? <>
                {button(copy.start, { action: 'start' })}
                {button(copy.stop, { action: 'stop', expected })}
                {button(copy.restart, { action: 'restart', expected })}
                {button(copy.update, { action: 'update', expected, settings: {} })}
                <Button variant="secondary" size="sm" label={copy.edit} isDisabled={busy}
                  onClick={() => setEdit({ expected, settings: {
                    mode: deployment.mode, websocket: deployment.websocket,
                    projectDirectoryRoots: deployment.projectDirectoryRoots ?? null,
                  } })} />
              </> : null}
              {status?.kind === 'installed' && status.pendingUpdate && expected
                ? button(copy.reconcile, { action: 'reconcile', expected }) : null}
              {deployment ? button(copy.logs, { action: 'logs' }) : null}
              {expected ? <Button variant="secondary" size="sm" label={copy.uninstall}
                isDisabled={busy} onClick={() => setConfirmUninstall(expected)} /> : null}
            </div>
            {confirmUninstall ? <div className="settingsRuntimeHostManagementActions">
              {button(copy.confirm, { action: 'uninstall', expected: confirmUninstall })}
              <Button variant="secondary" size="sm" label={copy.cancel} isDisabled={busy}
                onClick={() => setConfirmUninstall(undefined)} />
            </div> : null}
            {edit ? <>
              {conflict ? <Banner status="warning" title={copy.conflict} /> : null}
              <Selector label={copy.mode} value={edit.settings.mode ?? 'on_demand'} isDisabled={busy}
                options={[{ value: 'on_demand', label: copy.onDemand }, { value: 'supervised', label: copy.supervised }]}
                onChange={(mode) => {
                  if (mode === 'on_demand' || mode === 'supervised') {
                    setEdit({ ...edit, settings: { ...edit.settings, mode } });
                  }
                }} />
              <TextInput label={copy.address} value={edit.settings.websocket ?? '127.0.0.1:0'} isDisabled={busy}
                onChange={(websocket) => setEdit({ ...edit, settings: { ...edit.settings, websocket } })} />
              <Selector label={copy.directories} value={edit.settings.projectDirectoryRoots == null ? 'default' : 'custom'}
                isDisabled={busy} options={[{ value: 'default', label: copy.default }, { value: 'custom', label: copy.custom }]}
                onChange={(policy) => setEdit({ ...edit, settings: {
                  ...edit.settings, projectDirectoryRoots: policy === 'default' ? null : [],
                } })} />
              {edit.settings.projectDirectoryRoots != null ? <RuntimeHostProjectDirectoryEditor
                roots={edit.settings.projectDirectoryRoots.map((root, id) => ({ ...root, id }))}
                isDisabled={busy} nextId={() => edit.settings.projectDirectoryRoots?.length ?? 0}
                copy={getSettingsProjectsCopy(locale).runtimeHost}
                onChange={(roots) => setEdit({ ...edit, settings: {
                  ...edit.settings, projectDirectoryRoots: roots.map(({ label, path }) => ({ label, path })),
                } })} /> : null}
              <div className="settingsRuntimeHostManagementActions">
                <Button variant="primary" size="sm" label={copy.save} isDisabled={busy || !!conflict}
                  onClick={() => void run({ action: 'update', expected: edit.expected, settings: edit.settings })} />
                <Button variant="secondary" size="sm" label={copy.cancel} isDisabled={busy}
                  onClick={() => setEdit(undefined)} />
              </div>
            </> : null}
            {result?.logs?.kind === 'not_captured' ? <p>{copy.notCaptured}</p> : null}
            {result?.logs?.kind === 'tail' ? <>
              {result.logs.byteTruncated ? <p>{copy.truncated}</p> : null}
              <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{result.logs.text}</pre>
            </> : null}
          </div>
        </LayoutContent>} />
    </Dialog>
  );
}
