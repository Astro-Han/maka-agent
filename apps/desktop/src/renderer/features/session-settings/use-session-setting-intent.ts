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

import { useMemo } from 'react';
import type { OrchestrationMode } from '@maka/core/orchestration';
import type { SandboxMode } from '@maka/core/permission';
import { executionPoliciesEqual, type ExecutionPolicy, type ApprovalPolicy } from '@maka/core/execution-permissions';
import type { ThinkingLevel } from '@maka/core/model-thinking';
import {
  isChatDefaultSandboxMode,
} from '@maka/core/settings';
import {
  useSessionSettingIntent as useSharedSessionSettingIntent,
} from '@maka/ui';
import {
  equalSessionModelConfigurationIntent,
  modelConfigurationIntentForModel,
  modelConfigurationIntentForThinking,
  type SessionModelConfigurationIntent,
  type SessionModelTarget,
} from './session-model-configuration-intent.js';
import type { SessionCatalogController } from '../../application/contracts/session-catalog/session-catalog-state.js';
import { useSessionSettingsServices } from './services-context.js';

type SessionSettingValues = {
  modelConfiguration: SessionModelConfigurationIntent;
  executionPolicy: ExecutionPolicy;
  planMode: boolean;
  orchestrationMode: OrchestrationMode;
};

export function useSessionSettingIntent<Owner extends { sessionId?: string }>(input: {
  catalog: Pick<SessionCatalogController, 'getState' | 'subscribe'>;
  isActiveSession(sessionId: string): boolean;
  newTaskExecutionPolicy: ExecutionPolicy;
  refreshCatalog(): Promise<unknown>;
  saveComposerDefaults(model: SessionModelTarget): void;
  writeFailureCopy(
    setting: 'model' | 'thinking' | 'permission' | 'plan' | 'orchestration',
    error: unknown,
  ): { title: string; description: string };
  showSessionError(sessionId: string, title: string, description: string): void;
  planMode: {
    write(sessionId: string, active: boolean): Promise<boolean>;
  };
  captureOwner(): Owner;
  isOwnerActive(owner: Owner): boolean;
  setNewTaskExecutionPolicy(policy: ExecutionPolicy): void;
  confirmBypass(allProtections?: boolean): Promise<boolean>;
}) {
  const services = useSessionSettingsServices();
  const reportWriteError = (
    sessionId: string,
    error: unknown,
    setting: 'model' | 'thinking' | 'permission' | 'plan' | 'orchestration',
  ) => {
    if (!input.isActiveSession(sessionId)) return;
    const failure = input.writeFailureCopy(setting, error);
    input.showSessionError(sessionId, failure.title, failure.description);
  };
  const catalogSessionRevision = (sessionId: string) =>
    input.catalog.getState().sessions.find((session) => session.id === sessionId)?.revision;
  const catalog = useMemo(() => ({
    revision: () => input.catalog.getState().revision,
    subscribeChanged: input.catalog.subscribe,
  }), [input.catalog]);
  const intent = useSharedSessionSettingIntent<SessionSettingValues>({
    catalog,
    refreshCatalog: input.refreshCatalog,
    channels: {
      modelConfiguration: {
        isEqual: equalSessionModelConfigurationIntent,
        write: async (sessionId, configuration) => {
          const summary = await services.setModelConfiguration(sessionId, {
            ...configuration.modelTarget,
            thinkingLevel: configuration.thinkingLevel,
          });
          const committed =
            summary.llmConnectionId === configuration.modelTarget.llmConnectionId &&
            summary.llmConnectionSlug === configuration.modelTarget.llmConnectionSlug &&
            summary.model === configuration.modelTarget.model &&
            (summary.thinkingLevel ?? null) === configuration.thinkingLevel;
          if (committed && configuration.changedSetting === 'model') {
            input.saveComposerDefaults(configuration.modelTarget);
          }
          return { committed, sessionRevision: summary.revision };
        },
        catalogSessionRevision,
        onWriteError: (sessionId, error, attempted) =>
          reportWriteError(sessionId, error, attempted.changedSetting),
      },
      executionPolicy: {
        isEqual: executionPoliciesEqual,
        write: async (sessionId, policy) => {
          const summary = await services.setExecutionPolicy(sessionId, policy);
          return {
            committed: summary.approvalPolicy !== null && executionPoliciesEqual({
              sandboxMode: summary.sandboxMode, approvalPolicy: summary.approvalPolicy,
            }, policy),
            sessionRevision: summary.revision,
          };
        },
        catalogSessionRevision,
        onWriteError: (sessionId, error) => reportWriteError(sessionId, error, 'permission'),
      },
      planMode: {
        // Exiting a pending proposal returns Plan state rather than a Session
        // summary, so this policy channel has no authoritative Session revision.
        write: input.planMode.write,
        onWriteError: (sessionId, error) => reportWriteError(sessionId, error, 'plan'),
      },
      orchestrationMode: {
        write: async (sessionId, mode) => {
          const summary = await services.setOrchestrationMode(sessionId, mode);
          return {
            committed: summary.orchestrationMode === mode,
            sessionRevision: summary.revision,
          };
        },
        catalogSessionRevision,
        onWriteError: (sessionId, error) =>
          reportWriteError(sessionId, error, 'orchestration'),
      },
    },
  });

  const sessionPolicy = (sessionId: string): ExecutionPolicy | undefined => {
    const pending = intent.overlayByChannel.executionPolicy[sessionId];
    if (pending) return pending;
    const session = input.catalog.getState().sessions.find((candidate) => candidate.id === sessionId);
    return session?.approvalPolicy
      ? { sandboxMode: session.sandboxMode, approvalPolicy: session.approvalPolicy }
      : undefined;
  };

  return {
    clear: intent.clear,
    abandonPlanProposal: services.abandonPlanProposal,
    setCollaborationMode: services.setCollaborationMode,
    overlays: intent.overlayByChannel,
    setApprovalPolicy: (approvalPolicy: ApprovalPolicy) => {
      const owner = input.captureOwner();
      if (!input.isOwnerActive(owner)) return Promise.resolve(false);
      if (!owner.sessionId) {
        input.setNewTaskExecutionPolicy({ ...input.newTaskExecutionPolicy, approvalPolicy });
        return Promise.resolve(true);
      }
      const current = sessionPolicy(owner.sessionId);
      return current ? intent.request('executionPolicy', owner.sessionId, { ...current, approvalPolicy })
        : Promise.resolve(false);
    },
    disableProtections: async () => {
      const owner = input.captureOwner();
      if (owner.sessionId && !sessionPolicy(owner.sessionId)) return false;
      if (!(await input.confirmBypass(true)) || !input.isOwnerActive(owner)) return false;
      const policy: ExecutionPolicy = {
        sandboxMode: 'danger-full-access', approvalPolicy: { kind: 'never' },
      };
      if (owner.sessionId) return intent.request('executionPolicy', owner.sessionId, policy);
      input.setNewTaskExecutionPolicy(policy);
      return true;
    },
    setSessionModel: (sessionId: string, modelTarget: SessionModelTarget) =>
      intent.request('modelConfiguration', sessionId, modelConfigurationIntentForModel(modelTarget)),
    setSessionThinkingLevel: (sessionId: string, thinkingLevel: ThinkingLevel | null) => {
      const pending = intent.overlayByChannel.modelConfiguration[sessionId];
      const session = input.catalog.getState().sessions.find((candidate) => candidate.id === sessionId);
      const currentModelTarget = session?.llmConnectionId
        ? {
            llmConnectionId: session.llmConnectionId,
            llmConnectionSlug: session.llmConnectionSlug,
            model: session.model,
          }
        : undefined;
      const next = modelConfigurationIntentForThinking(currentModelTarget, pending, thinkingLevel);
      return next
        ? intent.request('modelConfiguration', sessionId, next)
        : Promise.resolve(false);
    },
    setSandboxMode: async (mode: SandboxMode) => {
      if (!isChatDefaultSandboxMode(mode)) return false;
      const owner = input.captureOwner();
      const sessionId = owner.sessionId;
      const overlay = sessionId ? intent.overlayByChannel.executionPolicy[sessionId] : undefined;
      const currentPolicy = sessionId ? sessionPolicy(sessionId) : undefined;
      if (sessionId && !currentPolicy) return false;
      const currentMode = sessionId
        ? currentPolicy?.sandboxMode
        : input.newTaskExecutionPolicy.sandboxMode;
      if (currentMode === mode) {
        return sessionId && overlay !== undefined
          ? intent.request('executionPolicy', sessionId, overlay)
          : true;
      }
      if (mode === 'danger-full-access' && !(await input.confirmBypass())) return false;
      if (!input.isOwnerActive(owner)) return false;
      if (sessionId && currentPolicy) {
        return intent.request('executionPolicy', sessionId, { ...currentPolicy, sandboxMode: mode });
      }
      input.setNewTaskExecutionPolicy({ ...input.newTaskExecutionPolicy, sandboxMode: mode });
      return true;
    },
    setPlanMode: (sessionId: string, active: boolean) =>
      intent.request('planMode', sessionId, active),
    setOrchestrationMode: (sessionId: string, mode: OrchestrationMode) =>
      intent.request('orchestrationMode', sessionId, mode),
  };
}
