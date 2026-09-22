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

import { resolveDesktopWslHostHandoff } from './runtime-host-wsl-handoff.js';
import { pluginNotifications } from './plugin-notifications.js';
import {
  app,
  type BrowserWindow,
  clipboard,
  ipcMain,
  Menu,
  nativeImage,
  nativeTheme,
  Notification,
  powerMonitor,
  powerSaveBlocker,
  shell,
  Tray,
  type MessageBoxOptions,
  type MessageBoxReturnValue,
} from "electron";
import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  createNativeRuntimeHostCandidateLaunchBarrier,
  initializeNativeRuntimeHost,
} from "./native-runtime-host.js";
import { type ConnectionEvent } from '@maka/core/connections';
import { type SessionChangedEvent, type SessionChangedReason } from '@maka/core/session';
import { isBotDeliveryProvider } from '@maka/core/bot-chat-settings';
import { resolveSystemUiLocale } from '@maka/core/ui-locale';
import {
  PROVIDER_REGISTRY,
  providerAuthRequiresSecret,
} from "@maka/core/llm-connections";
import { BotRegistry, type BotIncomingMessage } from '@maka/runtime/bots';
import { buildMcpToolsWithIdentities } from '@maka/runtime/mcp-tools';
import {
  createClientRuntimeHostCredentialStore,
  createClientRuntimeHostProfileCatalog,
  createRuntimeHostCandidateLaunchBarrier,
  LOCAL_RUNTIME_HOST_PROFILE,
  loadOrCreateRuntimeHostClientInstanceId,
  listRuntimeHostWslDistributions,
  runtimeHostProfileAccess,
  RuntimeHostProfileConnectionError,
  type ResolvedRuntimeHostProfile,
} from "@maka/runtime-host/client";
import {
  openRuntimeHostPeerMeshComponent,
  type RuntimeHostPeerMeshComponent,
} from '@maka/runtime-host/peer-mesh';
import {
  openRuntimeHostPeerEndpointOwner,
  type RuntimeHostPeerEndpointOwner,
} from '@maka/runtime-host/peer-reachability';
import { clientCapabilityEntityId } from "@maka/runtime-host/client-capability-entity-id";
import type { WorkspaceTarget } from "@maka/runtime-host/protocol";
import { runtimeHostProfileUsesHostWorkspace } from "@maka/runtime-host/profile-kind";
import { createCredentialMcpOAuthStorage, McpClientManager } from "@maka/mcp";
import { createWorkBoardStore } from "@maka/storage/work-board-store";
import { normalizeWorkBoardLinkedSession } from "@maka/core/work-board";
import { createFileCredentialStore } from "@maka/storage/credential-store";
import { createMcpConfigStore } from "@maka/storage/mcp-config-store";
import { createSettingsStore } from "@maka/storage/settings-store";
import { resolveStorageRoot } from "@maka/storage/root-authority";

import { createMcpOAuthController } from "./mcp-oauth-controller.js";
import { CommandCodeBrowserLoginController } from "./commandcode-browser-login.js";
import { registerCommandCodeLoginIpc } from "./commandcode-login-ipc-main.js";
import { createWorkHubControl } from './workhub-control.js';
import { createWorkHubPresentation } from './workhub-presentation.js';
import { createWorkHubRuntime } from './workhub-runtime.js';
import { createWindowsAppTray } from './windows-app-tray.js';
import { readableAppIconPath } from './app-icon-surface.js';
import { registerAppClientIpc, registerAppIpc } from "./app-ipc-main.js";
import { createAppQuitCoordinator } from "./app-quit-coordinator.js";
import {
  desktopDiagnosticUpdateChannel,
  desktopUpdateChannelFromManifest,
  verifyDownloadedUpdateAttestation,
} from "./app-update-attestation.js";
import { createAppUpdateService } from "./app-update-service.js";
import { createAttachmentApprovalRegistry } from "./attachment-approval.js";
import { renderAttachmentPreview, resizeImageForAttachment } from "./attachment-resize-native.js";
import { registerAttachmentPreviewIpc } from "./attachment-preview.js";
import { readFileCapped, resolvePickedAttachments } from "./attachment-ingest.js";
import { DesktopSessionLocalStore } from './session-local-store.js';
import { DesktopSessionLocalService, desktopSessionLocalPartition, registerDesktopSessionLocalIpc, type DesktopSessionLocalTarget } from './session-local-service.js';
import { registerBrowserIpc } from "./browser-ipc-main.js";
import { browserViewHost } from "./browser/browser-host.js";
import { releaseBrowserSession } from "./browser/session.js";
import {
  isBrowserMessageBoxPresentationActive,
  showBrowserMessageBox,
  type BrowserMessageBoxTheme,
} from "./browser-message-box.js";
import { createE2eFixtureBotOnboardingAdapters } from "./bot-onboarding-e2e-fixture.js";
import { resolveBuildInfo } from "./build-info.js";
import { computerUseServiceHealth } from "./computer-use-host.js";
import { registerDesktopDiagnosticsIpc } from "./desktop-diagnostics-ipc-main.js";
import { assembleDesktopNativeCapabilities } from "./desktop-native-capability-assembly.js";
import { clientSettingsConfirmation } from "./client-settings-confirmation-copy.js";
import { nativeFileDialogCopy } from "./native-file-dialog-copy.js";
import { createDesktopLocaleAuthority } from "./desktop-locale-authority.js";
import { buildRiveWorkflowTool } from "./rive-workflow-tool.js";
import { applyAppIcon } from "./app-icon-surface.js";
import { registerAppIconIpc } from "./app-icon-ipc.js";
import { listAppIconPreviews } from "./app-icon-surface.js";
import { importCustomAppIcon } from "./custom-app-icons.js";
import { installDesktopShellPresentation } from "./desktop-shell-presentation.js";
import {
  resolveE2eFixture,
  seedE2eFixture,
} from "./e2e-fixture.js";
import { PARTIAL_HISTORY_TRANSCRIPT_BYTES } from "./e2e-fixture/seed-helpers.js";
import { createKeepSystemAwakeController } from "./keep-system-awake.js";
import { isDarkAppearance } from "./theme-source.js";
import {
  readWithFallback,
  type ReconnectableReadIpcMain,
} from "./ipc-reconnect-policy.js";
import { createMainWindowController } from "./main-window.js";
import type { DesktopRuntimeHostIdentity } from "../preload/bridge-contract.js";
import {
  captureDesktopDiagnosticEnvironment,
  copyDesktopDiagnosticReport,
  createDesktopMainRendererDiagnosticInput,
  createDesktopStartupDiagnosticInput,
  mainProcessLogBuffer,
  runtimeHostProcessLogBuffer,
  type DesktopDiagnosticsDeps,
} from "./main-process-diagnostics.js";
import {
  defaultRuntimeHostRecoveryDialog,
  showMainRendererProcessGoneDialog,
  showMessageBoxWithDiagnostics,
} from "./native-diagnostic-dialog.js";
import { getNativeDiagnosticDialogCopy } from "./native-diagnostic-dialog-copy.js";
import {
  resolveDesktopSessionWorkspace,
} from "./new-session-project.js";
import { createMcpExclusiveLane, registerMcpIpcMain } from "./mcp-ipc-main.js";
import { createOnboardingService } from "./onboarding-service.js";
import { registerOnboardingIpc } from "./onboarding-ipc-main.js";
import {
  createDesktopTaskSubmissionReadinessService,
  registerTaskSubmissionReadinessIpc,
  type DesktopModelTargetResolution,
} from "./task-submission-readiness-main.js";
import { registerNotificationsIpc } from "./notifications-ipc-main.js";
import { registerMarkdownSaveIpc } from "./markdown-save-ipc-main.js";
import { registerPetPackIpc } from "./pet-pack-import.js";
import { registerWorkBoardIpc } from "./work-board-ipc-main.js";
import {
  createPermissionOverlayMain,
  registerPermissionOverlayIpc,
} from "./permission-overlay/permission-overlay-main.js";
import { resolveProjectContextRoot } from "./project-context-root.js";
import { createProjectManagementService } from "./project-management-service.js";
import { projectPickerTitle } from "./project-picker-copy.js";
import type { ProjectManagementService } from "./project-management-service.js";
import {
  createProjectRootController,
  type ProjectRootController,
} from "./project-root-controller.js";
import { createSessionCopyCleanupAuthority } from "@maka/storage/session-copy-cleanup";
import {
  projectHostConnections,
  registerRuntimeHostConnectionsIpc,
} from "./runtime-host-connections-ipc-main.js";
import { registerRuntimeHostConfigIpc } from "./runtime-host-config-ipc-main.js";
import { createCapabilityRevisionPublisher } from "./runtime-host-capability-revision-publisher.js";
import { buildClientSettingsTools } from "./client-settings-tools.js";
import { createClientSettingsEffects } from "./client-settings-effects.js";
import { registerClientSettingsIpc } from "./client-settings-ipc-main.js";
import { startClientSettingsWatcher } from "./client-settings-watcher.js";
import { registerRuntimeHostGitHubCopilotIpc } from "./runtime-host-github-copilot-ipc-main.js";
import { registerRuntimeHostArtifactsIpc } from "./runtime-host-artifacts-ipc-main.js";
import { ManagedArtifactPreview } from './managed-artifact-preview.js';
import { buildManagedArtifactPreviewTools } from './managed-artifact-preview-tools.js';
import type { DesktopRuntimeHostClient } from "./runtime-host-client.js";
import type {
  DesktopRuntimeHostCandidateControls,
  DesktopRuntimeHostTargetPolicy,
} from "./runtime-host-desktop-candidate.js";
import {
  RuntimeHostUpgradeCancelledError,
  startRuntimeHostDesktopManager,
  type RuntimeHostDesktopManager,
  type RuntimeHostDesktopTargetState,
} from "./runtime-host-desktop-manager.js";
import {
  buildRuntimeHostActiveQuitDialog,
} from "./runtime-host-quit-copy.js";
import { prepareRuntimeHostQuit } from "./runtime-host-quit.js";
import { createDesktopHostHandoffSurface } from './startup-presentation.js';
import { registerRuntimeHostMemoryIpc } from "./runtime-host-memory-ipc-main.js";
import {
  createDesktopRuntimeHostProfileService,
  registerDesktopRuntimeHostProfileIpc,
  resolveDesktopRuntimeHostStartup,
} from "./runtime-host-profile-service.js";
import {
  createDesktopGuestSessionMountService,
  createGuestSessionMountStore,
  registerDesktopGuestSessionMountIpc,
} from './runtime-host-guest-session-mounts.js';
import {
  createDesktopRuntimeHostSshTerminal,
} from "./runtime-host-ssh-terminal.js";
import {
  runDesktopRuntimeHostWslManagement,
  resolveDesktopRuntimeHostWslTarget,
  runNativeRuntimeHostWslSetup,
  prepareNativeRuntimeHostWslPackage,
} from './runtime-host-wsl-controller.js';
import { nativeRuntimeHostVersion, resolveNativeRuntimeHostPackage } from './native-runtime-host-setup.js';
import {
  createRuntimeHostSetupPackageResolver,
} from "./runtime-host-setup-package.js";
import {
  configureDesktopRuntimeHostPeerClient,
  readDesktopRuntimeHostWebRtcStunPolicy,
  writeDesktopRuntimeHostWebRtcStunPolicy,
} from './runtime-host-peer-client.js';
import { createDesktopRuntimeHostLocalOperator } from './runtime-host-local-operator.js';
import { createDesktopLocalRuntimeHostRemoteAccess } from './runtime-host-local-remote-access.js';
import { NativeHostBudget } from './native-runtime-host-operation.js';
import { createDesktopRuntimeHostOnboarding } from "./runtime-host-onboarding.js";
import { createDesktopRuntimeHostManagement } from "./runtime-host-management.js";
import { createNativeRuntimeHostManagement } from './native-runtime-host-management.js';
import { nativeRuntimeHostManagementRequestSchema } from '../shared/native-runtime-host-management.js';
import type { NativeRuntimeHostOperator } from './native-runtime-host-command.js';
import { createDesktopRuntimeHostLocalManagement } from './runtime-host-local-management.js';
import { createDesktopRuntimeHostPeerMeshManagement } from './runtime-host-peer-mesh-management.js';
import { registerExternalAgentSetupIpc } from "./external-agent-setup-ipc-main.js";
import { registerRuntimeHostOAuthIpc } from "./runtime-host-oauth-ipc-main.js";
import { RuntimeHostOAuthPresentation } from "./runtime-host-oauth-presentation.js";
import { registerRuntimeHostPermissionsIpc } from "./runtime-host-permissions-ipc-main.js";
import { registerRuntimeHostRendererIpc } from "./runtime-host-renderer-ipc-main.js";
import { registerClientPluginRemoteIpc } from './client-plugin-remote-ipc.js';
import { pluginAuthorizationDialog } from './plugin-authorization-dialog.js';
import { sandboxSetupDialog, sandboxSetupUnavailable } from './sandbox-setup.js';
import { registerRuntimeHostSearchIpc } from "./runtime-host-search-ipc-main.js";
import { createRuntimeHostProjectCatalog } from "./runtime-host-project-catalog.js";
import { createRuntimeHostDefaultRecovery } from "./runtime-host-default-recovery.js";
import { toDesktopHostSessionSummary } from "./runtime-host-session-catalog-ipc-main.js";
import {
  createRuntimeHostSettingsModule,
  registerRuntimeHostSettingsIpc,
} from "./runtime-host-settings-ipc-main.js";
import { registerRuntimeHostUsageIpc } from "./runtime-host-usage-ipc-main.js";
import { registerRuntimeHostWorkspaceIpc } from "./runtime-host-workspace-ipc-main.js";
import { resolveShellEnv } from "./shell-env.js";
import {
  registerSettingsBotsIpc,
  type SettingsBotsIpcHandle,
} from "./settings-bots-ipc-main.js";
import {
  isComputerUseRealModelE2e,
  isE2e,
  isIsolatedE2e,
  revealMode,
} from "./startup-context.js";
import { resolveDesktopStorageRoot } from "./storage-root-startup.js";
import { startupStep } from "./startup-step.js";
import {
  closeDesktopStartupProgress,
  desktopStartupProgressWindow,
  isDesktopStartupInProgress,
  updateDesktopStartupProgress,
} from './startup-presentation.js';
import { registerWorkspaceSearchIpc } from "./workspace-search-ipc-main.js";
import {
  parseDesktopSessionResourceKey,
  requireDesktopTargetScope,
  type DesktopTargetScope,
} from "../shared/runtime-host-identity.js";

await resolveShellEnv();

const MANAGED_UPDATE_RECONNECT_TIMEOUT_MS = 10_000;
const buildInfo = resolveBuildInfo(app.isPackaged, app.getAppPath());
const userDataDir = app.getPath("userData");
const runtimeHostPeerConfiguration = await configureDesktopRuntimeHostPeerClient({
  isPackaged: app.isPackaged,
  enableDevelopmentPeer: process.argv.includes('--runtime-host-peer'),
  appPath: app.getAppPath(),
  resourcesPath: process.resourcesPath,
  clientDataRoot: userDataDir,
});
let runtimeHostPeerEndpointOwner: RuntimeHostPeerEndpointOwner | undefined;
let runtimeHostPeerMeshComponent: RuntimeHostPeerMeshComponent | undefined;
let runtimeHostPeerMesh: RuntimeHostPeerMeshComponent['mesh'] | undefined;
let runtimeHostPeerClient: RuntimeHostPeerEndpointOwner['client'] | undefined;
if (runtimeHostPeerConfiguration) {
  try {
    runtimeHostPeerEndpointOwner = await openRuntimeHostPeerEndpointOwner({
      ...runtimeHostPeerConfiguration,
      dataRoot: join(userDataDir, 'peer-mesh'),
      onBackgroundReachabilityError: (error) => {
        console.error('[runtime-host] peer reachability publication failed:', error);
      },
    });
    runtimeHostPeerClient = runtimeHostPeerEndpointOwner.client;
    void runtimeHostPeerEndpointOwner.closed.catch((error) => {
      console.error('[runtime-host] peer reachability publisher stopped:', error);
    });
    try {
      runtimeHostPeerMeshComponent = await openRuntimeHostPeerMeshComponent({
        dataRoot: join(userDataDir, 'peer-mesh'),
        endpoint: runtimeHostPeerEndpointOwner,
        endpointKind: 'client',
        onBackgroundReconcileError: (error) => {
          console.error('[runtime-host] Peer Mesh background synchronization failed:', error);
        },
      });
      runtimeHostPeerMesh = runtimeHostPeerMeshComponent.mesh;
      void runtimeHostPeerMeshComponent.closed.catch((error) => {
        runtimeHostPeerMesh = undefined;
        console.error('[runtime-host] Peer Mesh stopped; Direct peer remains available:', error);
      });
    } catch (error) {
      console.error('[runtime-host] Peer Mesh is unavailable; continuing with Direct peer:', error);
    }
  } catch (error) {
    console.error('[runtime-host] Direct peer is unavailable:', error);
  }
}
const runtimeHostDirectPeerAvailable = runtimeHostPeerClient !== undefined;
const runtimeHostClientInstanceId = await loadOrCreateRuntimeHostClientInstanceId(
  join(userDataDir, "runtime-host-client.json"),
);
const nativeHostBinaryName = process.platform === "win32" ? "maka.exe" : "maka";
const nativeHostExecutable = app.isPackaged
  ? join(process.resourcesPath, "runtime-host", nativeHostBinaryName)
  : fileURLToPath(new URL(`../../../../target/debug/${nativeHostBinaryName}`, import.meta.url));
const runtimeHostCandidateLaunchBarrier = isE2e
  ? createRuntimeHostCandidateLaunchBarrier()
  : createNativeRuntimeHostCandidateLaunchBarrier(nativeHostExecutable, ensureLocalStorageRoot);
const runtimeHostCredentialStore = createClientRuntimeHostCredentialStore(userDataDir);
const runtimeHostProfileCatalog = createClientRuntimeHostProfileCatalog(
  userDataDir,
  runtimeHostCredentialStore,
);
const runtimeHostStartup = await resolveDesktopRuntimeHostStartup(userDataDir, {
  catalog: runtimeHostProfileCatalog,
  credentialStore: runtimeHostCredentialStore,
});
let runtimeHostManager: RuntimeHostDesktopManager | undefined;
function activeRuntimeHostRef(): DesktopTargetScope | undefined {
  const current = runtimeHostManager?.current();
  return current?.hostId
    ? { hostId: current.hostId, targetEpoch: current.epoch }
    : undefined;
}
const runtimeHostGeneration = app.isPackaged ? app.getVersion() : randomUUID();
const e2eFixture = resolveDesktopE2eFixture();
const useBotOnboardingFixture = e2eFixture?.scenario === "settings-bots-onboarding";
const workspaceRoot = join(
  userDataDir,
  "workspaces",
  e2eFixture?.workspaceName ?? "default",
);
// Runtime authority and Desktop-owned control tables must not migrate the old TS Host database.
const localHostRoot = isE2e ? workspaceRoot : join(userDataDir, "runtime-host-rust");
const desktopControlRoot = isE2e ? workspaceRoot : join(userDataDir, "desktop-state");
const desktopDatabaseOptions = {
  schemaMigration: isE2e ? "require_current" as const : "migrate" as const,
};
mkdirSync(workspaceRoot, { recursive: true });
const desktopDiagnostics: DesktopDiagnosticsDeps = {
  environment: () =>
    captureDesktopDiagnosticEnvironment({
      appVersion: app.getVersion(),
      buildMode: buildInfo.mode,
      updateChannel: desktopDiagnosticUpdateChannel({
        isPackaged: app.isPackaged,
        appPath: app.getAppPath(),
      }),
      buildCommit: buildInfo.commit,
      locale: app.getLocale(),
      workspacePath: workspaceRoot,
    }),
  mainLogs: () => mainProcessLogBuffer.snapshot(),
  runtimeHostProcessLogs: () => runtimeHostProcessLogBuffer.snapshot(),
  runtimeHostConnections: () => runtimeHostManager?.entries() ?? [],
  resolveActiveRuntimeHost: () => {
    const scope = activeRuntimeHostRef();
    return scope ? resolveRuntimeHostDiagnostics(scope) : undefined;
  },
  resolveRuntimeHost: resolveRuntimeHostDiagnostics,
  writeClipboard: (report) => clipboard.writeText(report),
};
let resolveBrowserDialogParent = desktopStartupProgressWindow;
let resolveBrowserDialogAppearance = async (): Promise<BrowserMessageBoxTheme> => ({
  locale: resolveSystemUiLocale(app.getPreferredSystemLanguages()),
  palette: "default",
});

async function showDesktopMessageBox(
  options: MessageBoxOptions,
  override?: Partial<BrowserMessageBoxTheme>,
): Promise<MessageBoxReturnValue> {
  const appearance = { ...(await resolveBrowserDialogAppearance()), ...override, revealMode };
  return showBrowserMessageBox(options, resolveBrowserDialogParent(), appearance);
}

function showStartupDiagnosticDialog(
  options: MessageBoxOptions,
  locale: ReturnType<typeof resolveSystemUiLocale>,
  diagnosticDetails = options.detail,
): Promise<MessageBoxReturnValue> {
  return showMessageBoxWithDiagnostics(options, {
    locale,
    showMessageBox: (nextOptions) => showDesktopMessageBox(nextOptions, { locale }),
    copyDiagnostics: () =>
      copyDesktopDiagnosticReport(
        desktopDiagnostics,
        createDesktopStartupDiagnosticInput({
          title: options.title || options.message,
          description: options.message,
          ...(diagnosticDetails ? { details: diagnosticDetails } : {}),
        }),
      ),
  });
}
if (e2eFixture) {
  console.log(
    `[e2e-fixture] scenario=${e2eFixture.scenario} workspace=${workspaceRoot}`,
  );
  await seedE2eFixture({ workspaceRoot, fixture: e2eFixture });
}
const resolveLocalStorageRoot = async () => {
  if (!isE2e) {
    await startupStep("native storage root", initializeNativeRuntimeHost(nativeHostExecutable, localHostRoot));
    return resolveStorageRoot({ path: localHostRoot, kind: "interactive" });
  }
  return (
  e2eFixture
    ? resolveStorageRoot({ path: workspaceRoot, kind: "interactive" })
    : startupStep(
        "storage root",
        resolveDesktopStorageRoot(workspaceRoot, {
          confirmRepair: () => confirmDesktopStorageRootRepair(workspaceRoot),
        }),
      )
  );
};
updateDesktopStartupProgress('storage');
let startupLocalStorageRoot = isE2e ? await resolveLocalStorageRoot() : undefined;
let rootInitialization: Promise<NonNullable<typeof startupLocalStorageRoot>> | undefined;
function ensureLocalStorageRoot(budget: import('./native-runtime-host-operation.js').NativeHostBudget) {
  if (startupLocalStorageRoot) return Promise.resolve(startupLocalStorageRoot);
  if (rootInitialization) return budget.wait(rootInitialization, 'root initialization');
  const pending = budget.wait((async () => {
    await initializeNativeRuntimeHost(nativeHostExecutable, localHostRoot, budget.signal, budget);
    const root = await resolveStorageRoot({ path: localHostRoot, kind: 'interactive' });
    startupLocalStorageRoot = root;
    return root;
  })(), 'root initialization');
  rootInitialization = pending;
  void pending.finally(() => { if (rootInitialization === pending) rootInitialization = undefined; }).catch(() => undefined);
  return pending;
}
if (isE2e && !startupLocalStorageRoot) {
  app.quit();
  await new Promise<never>(() => {});
  throw new Error("Desktop storage root resolution did not complete");
}
const settingsStore = createSettingsStore(workspaceRoot);
const desktopLocale = createDesktopLocaleAuthority({
  readSettings: () => settingsStore.get(),
  preferredSystemLanguages: () => app.getPreferredSystemLanguages(),
});
resolveBrowserDialogAppearance = async () => {
  try {
    const settings = await settingsStore.get();
    return {
      locale: desktopLocale.observe(settings),
      palette: settings.appearance.palette,
      dark: isDarkAppearance(
        e2eFixture?.theme ?? settings.appearance.theme,
        nativeTheme.shouldUseDarkColors,
      ),
    };
  } catch {
    return { locale: desktopLocale.current(), palette: "default" };
  }
};
const mcpConfigStore = createMcpConfigStore(workspaceRoot);
const mcpManager = new McpClientManager({
  clientName: "maka-desktop",
  clientVersion: app.getVersion(),
  oauthStorage: createCredentialMcpOAuthStorage(
    createFileCredentialStore(workspaceRoot),
  ),
});
// One lane shared by config transactions and login claims: "no login is
// active" checked inside a transaction cannot be invalidated by a claim
// landing between the check and the write.
const mcpExclusiveLane = createMcpExclusiveLane();
const mcpOAuthController = createMcpOAuthController({
  manager: mcpManager,
  claimLane: mcpExclusiveLane,
  openExternal: (url) => shell.openExternal(url),
  ensureReady: () => ensureMcpReady(),
  callbackPort: async (serverId) => {
    const server = (await mcpConfigStore.get()).mcpServers[serverId];
    return server && "url" in server ? server.oauth?.callbackPort : undefined;
  },
});
let mcpStartup: Promise<void> | undefined;
function ensureMcpReady(): Promise<void> {
  if (!mcpStartup) {
    const startup = mcpConfigStore
      .get()
      .then((config) => mcpManager.sync(config));
    mcpStartup = startup;
    void startup.catch(() => {
      if (mcpStartup === startup) mcpStartup = undefined;
    });
  }
  return mcpStartup;
}
const keepSystemAwake = createKeepSystemAwakeController(powerSaveBlocker);
let onMainWindowClose = (): void => {};
let onMainWindowClosed = (): void => {};
const mainWindowController = createMainWindowController({
  workspaceRoot,
  e2eFixture,
  settingsStore,
  revealMode,
  onClose: () => onMainWindowClose(),
  onClosed: () => onMainWindowClosed(),
  onShow: closeDesktopStartupProgress,
  onRendererProcessGone: async (details) => {
    const diagnosticInput = createDesktopMainRendererDiagnosticInput({
      title: "Maka main Renderer process exited unexpectedly",
      description: `Reason: ${details.reason}`,
      details: `Exit code: ${details.exitCode}`,
    });
    for (;;) {
      const locale = await desktopLocale.resolve();
      const decision = await showMainRendererProcessGoneDialog({
        locale,
        copyDiagnostics: () =>
          copyDesktopDiagnosticReport(desktopDiagnostics, diagnosticInput),
        // showBrowserMessageBox attaches only to a visible, non-minimized
        // parent. A pre-first-paint crash therefore gets a standalone window.
        showMessageBox: (options) => showDesktopMessageBox(options, { locale }),
      });
      if (decision !== "recover") break;
      if (await mainWindowController.reloadMainRenderer()) return;
      if (!mainWindowController.browserWindow()) break;
    }
    app.quit();
  },
});
resolveBrowserDialogParent = () => {
  const main = mainWindowController.browserWindow();
  return main?.isVisible() ? main : desktopStartupProgressWindow();
};
const runtimeHostSshTerminal = createDesktopRuntimeHostSshTerminal({
  ipcMain,
  send: (channel, event) => mainWindowController.send(channel, event),
});
const runtimeHostSetupPackage = createRuntimeHostSetupPackageResolver({
  isPackaged: app.isPackaged,
  appPath: app.getAppPath(),
  environment: process.env,
});
const localRuntimeHostOperator = createDesktopRuntimeHostLocalOperator();
const localRuntimeHostRemoteAccess = createDesktopLocalRuntimeHostRemoteAccess({
  ipcMain,
  clientDataRoot: userDataDir,
  rootPath: localHostRoot,
  get rootId() {
    if (!startupLocalStorageRoot) throw new Error('Local Host identity is not available yet; retry after connecting');
    return startupLocalStorageRoot.rootId;
  },
  directPeerAvailable: runtimeHostDirectPeerAvailable,
  manager: () => runtimeHostManager,
  resolveSetupPackage: async (signal) => {
    updateDesktopStartupProgress('package');
    const result = await runtimeHostSetupPackage.resolveForThisDesktop(signal);
    updateDesktopStartupProgress('checking');
    return result;
  },
  onUpdateProgress: updateDesktopStartupProgress,
  operator: localRuntimeHostOperator,
});
const native = assembleDesktopNativeCapabilities({
  isComputerUseRealModelE2e,
  locale: desktopLocale,
  keepSystemAwake,
  mainWindow: mainWindowController,
});
const riveWorkflowTool = buildRiveWorkflowTool();
const completeDesktopInteractionTurn = (sessionId: string): void => {
  workHubControl.complete(sessionId);
  native.computerUseOverlay.clearForSession(sessionId);
  native.computerUsePip.complete(sessionId);
  native.computerUseStatusItem.clearForSession(sessionId);
  native.computerUseScreenLock.clearForSession(sessionId);
  native.computerUseTools.clearSession(sessionId);
};
const releaseDesktopInteractionSession = (sessionId: string): void => {
  workHubControl.complete(sessionId);
  native.computerUseOverlay.clearForSession(sessionId);
  native.computerUsePip.clearForSession(sessionId);
  native.computerUseStatusItem.clearForSession(sessionId);
  native.computerUseScreenLock.clearForSession(sessionId);
  native.computerUseTools.clearSession(sessionId);
};
const permissionOverlay = createPermissionOverlayMain({
  resolveLocale: () => desktopLocale.resolve(),
});
onMainWindowClose = () => {
  native.computerUseOverlay.destroyAll();
  native.computerUsePip.destroyAll();
};
const attachmentApprovals = createAttachmentApprovalRegistry();
const sessionLocalStore = new DesktopSessionLocalStore(join(userDataDir, 'session-experience.sqlite'));
const localSessionChanged = (scope: DesktopTargetScope, sessionId?: string): void => {
  mainWindowController.send('session-local:changed', scope, { sessionId });
  mainWindowController.send('sessions:changed', scope, { reason: 'updated', ts: Date.now(), ...(sessionId ? { sessionId } : {}) });
};
const sessionLocal = new DesktopSessionLocalService(sessionLocalStore, {
  targets: () => (runtimeHostManager?.entries() ?? []).flatMap((state) => {
    if (state.readiness === 'unavailable' && state.error instanceof RuntimeHostProfileConnectionError && state.error.reason === 'credential_rejected') return [];
    const target = localSessionTarget(state);
    return target ? [target] : [];
  }),
  changed: localSessionChanged,
  onError: (error) => console.error('[session-local] background synchronization failed:', error),
});
registerDesktopSessionLocalIpc({
  ipcMain, service: sessionLocal, approvals: attachmentApprovals, resizeImage: resizeImageForAttachment,
  changed: localSessionChanged,
  resolveWorkspace: async (target, input) => {
    const context = runtimePolicyTargetsByEpoch.get(target.scope.targetEpoch);
    if (!context?.isActive()) throw new Error('Select a cached project before creating an offline task');
    return resolveDesktopSessionWorkspace(input, context.projectManagement, context.projectCatalog, {
      allowHostPath: !runtimeHostProfileUsesHostWorkspace(context.policy.kind),
    });
  },
});

function localSessionTarget(state: RuntimeHostDesktopTargetState): DesktopSessionLocalTarget | undefined {
  if (runtimeHostProfileAccess(state.target.profile) !== 'owner') return undefined;
  const hostId = state.readiness === 'ready' ? state.candidate.client.hostId
    : state.hostId ?? (state.target.profile.kind === 'local' ? startupLocalStorageRoot?.rootId : state.target.profile.rootId);
  if (!hostId) return undefined;
  const partition = desktopSessionLocalPartition({ profileId: state.target.profile.id, hostId, incarnation: state.target.profileIncarnationId, credential: state.target.credential });
  return { partition, scope: { hostId, targetEpoch: state.epoch }, profileId: state.target.profile.id,
    ...(state.readiness === 'ready' && state.candidate.client.lifecycleState === 'ready'
      ? { client: state.candidate.client, submit: (input) => state.candidate.submitLocalMessage(input) } : {}) };
}
const oauthPresentation = new RuntimeHostOAuthPresentation((url) => shell.openExternal(url));
// Desktop-local by construction: the Studio page posts the key to a loopback
// port beside the browser, so the listener cannot live in a (possibly remote) Host.
const commandCodeLoginController = new CommandCodeBrowserLoginController({
  openExternal: (url) => shell.openExternal(url),
});
const runtimeHostProfileService = createDesktopRuntimeHostProfileService({
  clientDataRoot: userDataDir,
  startup: runtimeHostStartup,
  catalog: runtimeHostProfileCatalog,
  credentialStore: runtimeHostCredentialStore,
  states: () => runtimeHostManager?.entries() ?? [],
  enable: async (target, sshInteraction, onPeerEndpoint) => {
    if (target.profile.kind === 'local') {
      if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
      return runtimeHostManager.enable(undefined);
    }
    if (target.profile.kind === 'remote' && !target.credential) {
      throw new Error('A remote Runtime Host profile requires an access credential');
    }
    if (!runtimeHostManager) throw new Error("Runtime Host manager is unavailable");
    await runtimeHostManager.enable(
      {
        profile: target.profile,
        ...(target.credential ? { credential: target.credential } : {}),
        ...(target.profile.kind === 'remote' && target.profile.transport.kind === "ssh"
          ? { sshInteraction }
          : {}),
      },
      onPeerEndpoint
        ? (status) => {
            if (status.peerEndpoint) onPeerEndpoint(status.peerEndpoint);
          }
        : undefined,
    );
  },
  disable: async (profileId) => {
    if (!runtimeHostManager) throw new Error("Runtime Host manager is unavailable");
    await runtimeHostManager.disable(profileId);
  },
  finalizePairing: async (profileId) => {
    if (!runtimeHostManager) throw new Error("Runtime Host manager is unavailable");
    await runtimeHostManager.finalizePairing(profileId);
  },
  setDefault: (profileId) => {
    if (!runtimeHostManager) throw new Error("Runtime Host manager is unavailable");
    runtimeHostManager.setDefaultProfile(profileId);
  },
});
const notifyGuestSessionMountsChanged = (): void => {
  mainWindowController.send('session-collaboration:mounts:changed');
};
const guestSessionMountService = createDesktopGuestSessionMountService({
  store: createGuestSessionMountStore(runtimeHostCredentialStore),
  mount: async (
    target,
    signal,
    onConnectionPhase,
    onPeerEndpoint,
    onSessionCatalogChanged,
  ) => {
    if (target.profile.kind !== 'remote' || !target.credential) {
      throw new Error('A shared Session requires a remote Guest target');
    }
    if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
    await runtimeHostManager.mountGuest(
      { profile: target.profile, credential: target.credential },
      onSessionCatalogChanged,
      signal,
      onConnectionPhase,
      (status) => {
        if (status.peerEndpoint) onPeerEndpoint?.(status.peerEndpoint);
      },
    );
  },
  finalizeAccess: async (mountId, signal, onAccessActivated, onFinalizationStarted) => {
    if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
    return runtimeHostManager.finalizeGuestAccess(mountId, signal, onAccessActivated, onFinalizationStarted);
  },
  getSharedSession: async (mountId) => {
    const current = runtimeHostManager?.current(mountId);
    if (!current?.candidate) {
      throw new Error('Shared Session Runtime Host is reconnecting');
    }
    return current.candidate.client.getSharedSession();
  },
  inspect: (mountId) => {
    const state = runtimeHostManager?.entries().find(
      (candidate) => candidate.target.profile.id === mountId,
    );
    if (!state) return undefined;
    return {
      readiness: state.readiness,
      ...(state.readiness !== 'ready' && state.error ? { error: state.error } : {}),
      ...(state.readiness === 'ready' && state.candidate.client.peerPath
        ? { peerPath: state.candidate.client.peerPath }
        : {}),
    };
  },
  onMountsChanged: notifyGuestSessionMountsChanged,
  wakeConnection: (mountId) => runtimeHostManager?.wakePeerRecovery(mountId),
  unmount: async (mountId) => {
    if (!runtimeHostManager) return;
    await runtimeHostManager.unmountGuest(mountId);
  },
});
const resolveNativePackage = async (identity: import('./runtime-host-target.js').RuntimeHostTargetIdentity, signal?: AbortSignal, budget?: NativeHostBudget, onProgress?: (progress: import('../shared/native-runtime-host-management.js').NativeHostProgress) => void) =>
  resolveNativeRuntimeHostPackage({
    executable: nativeHostExecutable, cache: join(userDataDir, 'native-cli'),
    version: await nativeRuntimeHostVersion({ isPackaged: app.isPackaged, appPath: app.getAppPath(), environment: process.env }),
    identity, signal, budget, onProgress, sourceCache: app.isPackaged ? undefined : process.env.MAKA_NATIVE_CLI_PACKAGES,
  });
const runtimeHostOnboarding = createDesktopRuntimeHostOnboarding({
  ipcMain,
  clientInstanceId: runtimeHostClientInstanceId,
  profiles: runtimeHostProfileService,
  nativeSetup: {
    resolvePackage: resolveNativePackage,
    ssh: runtimeHostSshTerminal.runNativeSetup,
    wsl: runNativeRuntimeHostWslSetup,
  },
  resolveWslTargetIdentity: resolveDesktopRuntimeHostWslTarget,
  listWslDistributions: listRuntimeHostWslDistributions,
  resolveSshTargetIdentity: runtimeHostSshTerminal.resolveTargetIdentity,
  send: (snapshot) =>
    mainWindowController.send("runtime-host-onboarding:changed", snapshot),
});
const nativeRuntimeHostManagement = async (request: unknown, budget: NativeHostBudget) => {
  if (isE2e) return null;
  const root = await ensureLocalStorageRoot(budget);
  return createNativeRuntimeHostManagement({
    onProgress: (progress) => mainWindowController.send('runtime-host-management:native-progress', 'local', progress),
    operator: { kind: 'local', executable: nativeHostExecutable },
    prepareUpdate: async () => ({ kind: 'local', executable: nativeHostExecutable }),
    rootId: root.rootId,
    rootPath: root.canonicalPath,
    change: (run, budget) => {
      if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
      return runtimeHostManager.runNativeHostChange({ profile: LOCAL_RUNTIME_HOST_PROFILE }, run, budget);
    },
  }).run(request, budget);
};
ipcMain.handle('runtime-host-management:native', async (_event, request: unknown, profileId: unknown = 'local') => {
  if (typeof profileId !== 'string' || !profileId || profileId.length > 128) {
    throw new Error('Invalid native Host profile ID');
  }
  const parsed = nativeRuntimeHostManagementRequestSchema.parse(request);
  const budget = new NativeHostBudget(parsed.action === 'status' ? 15_000 : 180_000);
  if (profileId === 'local') return nativeRuntimeHostManagement(parsed, budget);
  const target = await budget.wait(runtimeHostProfileCatalog.resolve(profileId), 'resolve Host profile');
  const profile = target.profile;
  let operator: NativeRuntimeHostOperator;
  if (profile.kind === 'environment' && profile.operator.kind === 'native') {
    operator = { kind: 'wsl', distribution: profile.provider.distribution, operator: profile.operator };
  } else if (profile.kind === 'remote' && profile.access !== 'session_guest' &&
    profile.transport.kind === 'ssh' && profile.transport.activation?.operator.kind === 'native') {
    operator = {
      kind: 'ssh', destination: profile.transport.destination,
      ...(profile.transport.sshPort === undefined ? {} : { sshPort: profile.transport.sshPort }),
      operator: profile.transport.activation.operator,
    };
  } else {
    throw new Error('This profile has no native Host operator');
  }
  return createNativeRuntimeHostManagement({
    onProgress: (progress) => mainWindowController.send('runtime-host-management:native-progress', profileId, progress),
    operator,
    prepareUpdate: async (budget) => {
      const progress = (value: import('../shared/native-runtime-host-management.js').NativeHostProgress) => mainWindowController.send('runtime-host-management:native-progress', profileId, value);
      if (operator.kind === 'ssh') {
        const target = { destination: operator.destination, sshPort: operator.sshPort };
        const identity = await budget.wait(runtimeHostSshTerminal.resolveTargetIdentity(target), 'target identity');
        const prepared = await runtimeHostSshTerminal.prepareNativePackage({
          ...target, budget, package: await resolveNativePackage(identity, undefined, budget, progress),
        });
        return { ...operator, operator: prepared };
      }
      if (operator.kind === 'wsl') {
        const target = { distribution: operator.distribution };
        const identity = await budget.wait(resolveDesktopRuntimeHostWslTarget(target), 'target identity');
        const prepared = await prepareNativeRuntimeHostWslPackage({
          ...target, budget, package: await resolveNativePackage(identity, undefined, budget, progress),
        });
        if (prepared.platform !== 'posix') throw new Error('WSL requires a POSIX native operator');
        return { ...operator, operator: { ...prepared, platform: 'posix' } };
      }
      return operator;
    },
    rootId: profile.rootId,
    change: (run, budget) => {
      if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
      return runtimeHostManager.runNativeHostChange(target, run, budget);
    },
  }).run(parsed, budget);
});

const localRuntimeHostManagement = createDesktopRuntimeHostLocalManagement({
  remoteAccess: localRuntimeHostRemoteAccess,
  operator: localRuntimeHostOperator,
  rootPath: localHostRoot,
  resolveUpdatePackage: () => runtimeHostSetupPackage.resolveForThisDesktop(),
  currentHostEpoch: () =>
    runtimeHostManager?.current('local')?.candidate?.client.hostEpoch,
  awaitUpdatedConnection: async (previousHostEpoch, replacementExpected) => {
    if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
    await runtimeHostManager.waitUntilReady(
      'local',
      replacementExpected ? previousHostEpoch : undefined,
      AbortSignal.timeout(MANAGED_UPDATE_RECONNECT_TIMEOUT_MS),
    );
  },
});
const runtimeHostManagement = createDesktopRuntimeHostManagement({
  ipcMain,
  profiles: runtimeHostProfileService,
  runServiceManagement: runtimeHostSshTerminal.runServiceManagement,
  runWslManagement: runDesktopRuntimeHostWslManagement,
  runPeerManagement: runtimeHostSshTerminal.runPeerManagement,
  directPeerClientAvailable: runtimeHostDirectPeerAvailable,
  runUpdate: runtimeHostSshTerminal.runUpdate,
  runUpdatePolicy: runtimeHostSshTerminal.runUpdatePolicy,
  runUpdateReconciliation: runtimeHostSshTerminal.runUpdateReconciliation,
  setupPackageMode: runtimeHostSetupPackage.mode,
  resolveSshTargetIdentity: runtimeHostSshTerminal.resolveTargetIdentity,
  resolveUpdatePackage: runtimeHostSetupPackage.resolve,
  currentHostEpoch: (profileId) =>
    runtimeHostManager?.current(profileId)?.candidate?.client.hostEpoch,
  liveHost: (profileId) => runtimeHostManager?.current(profileId)?.candidate?.client,
  awaitUpdatedConnection: async (
    profileId,
    expectedHostId,
    previousHostEpoch,
    replacementExpected,
  ) => {
    if (!runtimeHostManager) throw new Error('Runtime Host manager is unavailable');
    const manager = runtimeHostManager;
    const expectedPrevious = replacementExpected ? previousHostEpoch : undefined;
    const reconnectExactTarget = async () => {
      await runtimeHostProfileService.reconnect(profileId, expectedHostId);
      const current = manager.current(profileId);
      if (
        current?.hostId !== expectedHostId ||
        !current.candidate ||
        (expectedPrevious !== undefined && current.candidate.client.hostEpoch === expectedPrevious)
      ) {
        throw new Error('Desktop reconnected to an unexpected Runtime Host generation');
      }
    };
    if (replacementExpected && previousHostEpoch === undefined) {
      await reconnectExactTarget();
      return;
    }
    try {
      await manager.waitUntilReady(
        profileId,
        expectedPrevious,
        AbortSignal.timeout(MANAGED_UPDATE_RECONNECT_TIMEOUT_MS),
      );
      if (manager.current(profileId)?.hostId !== expectedHostId) {
        throw new Error('Runtime Host profile changed while its service was updating');
      }
    } catch {
      await reconnectExactTarget();
    }
  },
  sendProgress: (progress) =>
    mainWindowController.send("runtime-host-management:progress", progress),
  runAccessManagement: runtimeHostSshTerminal.runAccessManagement,
  cleanupManagedDeployment: runtimeHostSshTerminal.cleanupManagedDeployment,
  providers: [localRuntimeHostManagement],
});
const runtimeHostPeerMeshManagement = createDesktopRuntimeHostPeerMeshManagement({
  ipcMain,
  localMesh: () => runtimeHostPeerMesh,
  localHost: localRuntimeHostRemoteAccess,
  runLocal: localRuntimeHostOperator.runPeerMesh,
  liveHost: (profileId) => runtimeHostManager?.current(profileId)?.candidate?.client,
  profiles: runtimeHostProfileService,
  runRemote: runtimeHostSshTerminal.runPeerMeshManagement,
  readConnectivityPolicy: () => readDesktopRuntimeHostWebRtcStunPolicy(userDataDir),
  writeConnectivityPolicy: (policy) =>
    writeDesktopRuntimeHostWebRtcStunPolicy(userDataDir, policy),
});
const defaultRuntimeHostRecovery = createRuntimeHostDefaultRecovery({
  defaultProfileId: () =>
    runtimeHostManager?.defaultProfileId() ??
    runtimeHostStartup.preferences.defaultProfileId,
  prompt: promptForDefaultRuntimeHostRecovery,
  retry: async (profileId) => {
    try {
      const snapshot = await runtimeHostProfileService.setEnabled(profileId, true);
      const entry = snapshot.entries.find((candidate) => candidate.profile.id === profileId);
      return entry?.readiness === "unavailable"
        ? new Error(entry.message ?? "Runtime Host is unavailable")
        : undefined;
    } catch (error) {
      return error instanceof Error ? error : new Error(String(error));
    }
  },
  useLocal: async () => {
    await runtimeHostProfileService.setDefault(LOCAL_RUNTIME_HOST_PROFILE.id);
  },
  onError: (error) =>
    console.error("[runtime-host] default Host recovery failed:", error),
});
interface DesktopRuntimeHostTargetContext {
  readonly client: DesktopRuntimeHostClient;
  readonly policy: DesktopRuntimeHostTargetPolicy;
  readonly scope: DesktopTargetScope;
  readonly projectCatalog: ReturnType<typeof createRuntimeHostProjectCatalog>;
  readonly projectManagement: ProjectManagementService;
  readonly isActive: () => boolean;
}

const runtimePolicyTargets = new WeakMap<
  DesktopRuntimeHostTargetPolicy,
  DesktopRuntimeHostTargetContext
>();
const runtimePolicyTargetsByEpoch = new Map<string, DesktopRuntimeHostTargetContext>();
const selectedDesktopWorkspaceTarget = async (
  target: DesktopRuntimeHostTargetPolicy,
): Promise<WorkspaceTarget | undefined> => {
  const currentTarget = requireRuntimePolicyTarget(target);
  const current = await currentTarget.projectManagement.current();
  if (typeof current.projectId === "string") {
    return { kind: "project", projectId: current.projectId };
  }
  if (runtimeHostProfileUsesHostWorkspace(target.kind)) return undefined;
  return { kind: "host_path", path: current.path };
};
const currentDesktopWorkspaceTarget = async (
  target: DesktopRuntimeHostTargetPolicy,
): Promise<WorkspaceTarget> => {
  const workspace = await selectedDesktopWorkspaceTarget(target);
  if (!workspace) {
    throw new Error("Select a project from the Runtime Host first");
  }
  return workspace;
};
const requireWorkHubTarget = (scope: DesktopTargetScope): DesktopRuntimeHostTargetContext => {
  const target = runtimePolicyTargetsByEpoch.get(scope.targetEpoch);
  if (!target?.isActive() || target.scope.hostId !== scope.hostId) throw new Error('Runtime Host is unavailable');
  return target;
};
const isCurrentWorkHubTarget = (scope: DesktopTargetScope): boolean => {
  const target = runtimePolicyTargetsByEpoch.get(scope.targetEpoch);
  const current = runtimeHostManager?.current();
  return !!target?.isActive() && target.scope.hostId === scope.hostId &&
    current?.epoch === scope.targetEpoch && current.candidate?.client === target.client;
};
const workHubRuntime = createWorkHubRuntime({
  client: (scope) => requireWorkHubTarget(scope).client,
  isCurrent: isCurrentWorkHubTarget,
});
const workHubControl = createWorkHubControl({
  ipcMain,
  prepareWindow: (turnId) => workHubPresentation.prepareControl(turnId),
  finishControl: () => workHubPresentation.finishControl(),
  window: () => {
    const window = mainWindowController.browserWindow();
    if (!window) throw new Error('Maka window is unavailable');
    return window.webContents;
  },
  authorizedRenderer: (contents) => mainWindowController.ownsRenderer(contents),
  send: (channel, payload) => mainWindowController.send(channel, payload),
  readSettings: () => settingsStore.get(),
  client: (scope) => requireWorkHubTarget(scope).client,
  isCurrent: isCurrentWorkHubTarget,
  ...workHubRuntime,
});
const browserIpc = registerBrowserIpc({
  mainWindowController,
  isHostActive: (scope) => runtimeHostManager?.ownsScope(scope) === true,
});
let workHubEnabled = false;
const workHubPresentation = createWorkHubPresentation({
  isEnabled: () => workHubEnabled,
  revealMode,
  mainWindow: () => mainWindowController.browserWindow(),
  ensureMainWindow: async () => {
    await quitCoordinator.focusOrCreateWindow();
    const window = mainWindowController.browserWindow();
    if (!window || window.isDestroyed()) throw new Error('Maka window is unavailable');
    return window;
  },
  mainModuleDirectory: import.meta.dirname,
  viteDevServerUrl: process.env.VITE_DEV_SERVER_URL,
  preloadPath: join(import.meta.dirname, '..', 'preload', 'preload.cjs'),
  onViewCreated: (contents, container) => mainWindowController.registerAuxiliaryRenderer(contents, container),
  onVisibilityChanged: () => browserIpc.refreshVisibility(),
});
workHubPresentation.registerIpc();
const windowsAppTray = createWindowsAppTray({
  platform: process.platform,
  enabled: !e2eFixture && !isIsolatedE2e,
  locale: desktopLocale,
  createTray: () => {
    // Match the packaged app icon; 'default' is the legacy mascot artwork.
    const icon = nativeImage.createFromPath(readableAppIconPath('sky'));
    if (icon.isEmpty()) throw new Error('Maka tray artwork is unavailable');
    return new Tray(icon);
  },
  createMenu: (template) => Menu.buildFromTemplate(template),
  openMain: () => quitCoordinator.focusOrCreateWindow(),
  openWorkHub: () => workHubPresentation.show(),
  quit: () => app.quit(),
  onError: (error) => console.error('[tray]', error),
});
onMainWindowClosed = () => {
  // A hidden WorkHub host window can keep window-all-closed from firing.
  // Without a tray, use the existing quit flow; cancelling it restores Maka.
  if (process.platform !== 'darwin' && !windowsAppTray.hasTray() && !isDesktopStartupInProgress()) app.quit();
};
const mcpCapabilityPublisher = createCapabilityRevisionPublisher(() =>
  mcpManager.toolSnapshot().revision,
);
let settingsBotsIpc: SettingsBotsIpcHandle | undefined;
const botRegistry = new BotRegistry({
  onIncomingMessage: (message: BotIncomingMessage) => {
    void runtimeHostManager
      ?.handleBotIncomingMessage(message)
      .catch((error) => console.error("[runtime-host] bot message failed:", error));
  },
  onStatusChange: (status) => {
    mainWindowController.send("settings:bots:statusChanged", status);
  },
});
const clientSettingsEffects = createClientSettingsEffects({
  settingsStore,
  applyWorkHub: async (enabled) => {
    workHubEnabled = enabled;
    await workHubPresentation.refreshSettings();
  },
  applyKeepSystemAwake: async (enabled) => {
    keepSystemAwake.apply(enabled);
  },
  applyBotSettings: useBotOnboardingFixture
    ? async () => undefined
    : (settings) => botRegistry.applySettings(settings),
  applyAppIcon: async (icon) => {
    applyAppIcon(icon, (error) =>
      console.error("[icon] failed to apply the app icon:", error),
    );
  },
  systemPrefersDark: () => nativeTheme.shouldUseDarkColors,
  observeLocale: (settings) => desktopLocale.observe(settings),
  emitExternalChanged: () => {
    mainWindowController.send("settings:clientChanged");
    sendActiveRuntimeHostEvent("settings:externalChanged", { ts: Date.now() });
  },
});
// An OS appearance flip changes no setting, so nothing else would notice it.
// Only the icon depends on the answer, and `refresh` re-resolves it and
// no-ops when the resolved tile is the one already applied — which is the
// case for every user who has not set a separate dark icon.
nativeTheme.on("updated", () => {
  void clientSettingsEffects.refresh(false).catch((error) => {
    console.error("[icon] failed to re-apply the app icon after a theme change:", error);
  });
});

const clientSettingsTools = buildClientSettingsTools({
  read: () => settingsStore.get(),
  update: async (patch) => {
    const settings = await settingsStore.update(patch);
    await clientSettingsEffects.apply(settings, true);
    return settings;
  },
  confirm: async (changes) => {
    const locale = await desktopLocale.resolve();
    const copy = clientSettingsConfirmation(changes, locale);
    const result = await showDesktopMessageBox(
      {
        type: "question",
        message: copy.message,
        detail: copy.detail,
        buttons: copy.buttons,
        defaultId: 0,
        cancelId: 1,
        noLink: true,
      },
      { locale },
    );
    return result.response === 0;
  },
});
const managedArtifactPreview = new ManagedArtifactPreview();
const clientSettingsWatcher = startClientSettingsWatcher(
  workspaceRoot,
  () => {
    void clientSettingsEffects.refresh(true).catch((error) =>
      console.error("[runtime-host] Client settings refresh failed:", error),
    );
  },
  {
    onError: (error) =>
      console.error("[runtime-host] Client settings watcher failed:", error),
  },
);
const updateMockState =
  process.env.MAKA_UPDATE_MOCK_STATE === "available" ||
  process.env.MAKA_UPDATE_MOCK_STATE === "downloading" ||
  process.env.MAKA_UPDATE_MOCK_STATE === "downloaded"
    ? process.env.MAKA_UPDATE_MOCK_STATE
    : undefined;
const updateTestFeed = process.env.MAKA_UPDATE_TEST_FEED;
const desktopUpdateChannel = app.isPackaged
  ? desktopUpdateChannelFromManifest(
      JSON.parse(readFileSync(join(app.getAppPath(), "package.json"), "utf8")),
    )
  : "release";
const updateService = createAppUpdateService({
  currentVersion: app.getVersion(),
  isPackaged: app.isPackaged,
  updateChannel: desktopUpdateChannel,
  testFeedUrl: updateTestFeed,
  mockLatestVersion: process.env.MAKA_UPDATE_MOCK_VERSION,
  mockState: updateMockState,
  onStatusChange: (status) =>
    mainWindowController.send("app:updateStatusChanged", status),
  // The loopback-only upgrade harness owns synthetic bytes that cannot carry
  // a GitHub Actions identity, so it tests updater mechanics rather than
  // provenance. Ordinary packaged launches have no override and always reach
  // the Sigstore verifier below.
  verifyDownloadedUpdate: updateTestFeed
    ? async () => {}
    : ({ downloadedFile, version, files }) =>
        verifyDownloadedUpdateAttestation({
          channel: desktopUpdateChannel,
          downloadedFile,
          version,
          files,
          trustRootCacheDirectory: join(userDataDir, "update-trust", "sigstore"),
        }),
  prepareInstall: async (input) => {
    if (!runtimeHostManager) throw new Error("Runtime Host manager is unavailable");
    const retirement = await runtimeHostManager.retireOwnedLocalHost(
      input.allowInterruptActiveTasks ? "interrupt_active_work" : "refuse_active_work",
    );
    if (retirement.kind === "active_tasks") return retirement;
    return {
      kind: "prepared",
      rollback: retirement.kind === "retired" ? retirement.resume : () => {},
    };
  },
});
mcpManager.onChange(() => {
  sendActiveRuntimeHostEvent("mcp:changed", mcpManager.statuses());
  void mcpCapabilityPublisher.refreshIfChanged().catch((error) =>
    console.error("[runtime-host] MCP capability refresh failed:", error),
  );
});

registerPersistentClientIpc();
registerPetPackIpc({
  ipcMain,
  workspaceRoot,
  mainWindowController,
  settingsStore,
  resolveLocale: () => desktopLocale.resolve(),
});
registerNotificationsIpc({
  ipcMain,
  settingsStore,
  locale: desktopLocale,
  mainWindowController,
  e2e: isE2e,
});

const sessionCopyOwnerProcessId = randomUUID();
const startLocalRuntimeHostManager = () => startRuntimeHostDesktopManager(
  {
    rootPath: localHostRoot,
    nativeHost: !isE2e,
    clientInstanceId: runtimeHostClientInstanceId,
    generation: runtimeHostGeneration,
    candidateLaunchBarrier: runtimeHostCandidateLaunchBarrier,
    ...(runtimeHostPeerClient ? { peerClient: runtimeHostPeerClient } : {}),
    // The Desktop E2E composition lives behind its own entry module, which
    // release packaging drops: picking it here is what keeps FakeBackend and
    // the E2E bootstrap out of the shipped Runtime Host.
    candidateEntrypoint: isE2e
      ? new URL(import.meta.resolve("@maka/runtime-host/test-only/execution-candidate-e2e-main"))
      : "host",
    ipcMain,
    workspaceRoot,
    attachmentApprovals,
    stat: (path) => import("node:fs/promises").then(({ stat }) => stat(path)),
    resizeImage: resizeImageForAttachment,
    mainWindowController,
    nativeCapabilities: {
      browserTools: native.browserTools,
      resolveBrowserUrl: ({ sessionId, toolName, arguments: args }) => {
        if (toolName === "browser_navigate") {
          if (typeof args.url !== "string") {
            throw new Error("Browser navigation URL is unavailable");
          }
          return args.url;
        }
        const url = browserViewHost().currentUrl(sessionId);
        if (!url) throw new Error("Browser session has no current URL");
        return url;
      },
      releaseBrowserSession,
      computerUseTools: native.computerUseTools,
      additionalGroups: (scope) => {
        const mcpTools = buildMcpToolsWithIdentities(mcpManager);
        const mcpServers = new Map<string, typeof mcpTools>();
        for (const identified of mcpTools) {
          const server = mcpServers.get(identified.serverId);
          if (server) server.push(identified);
          else mcpServers.set(identified.serverId, [identified]);
        }
        return [
          workHubControl.group(scope),
          {
            offerId: 'desktop_artifact_preview',
            label: 'HTML Artifact preview',
            description: 'Prepare an isolated, temporary HTTP preview of a generated HTML Artifact.',
            tools: buildManagedArtifactPreviewTools(async (sessionId, artifactId, signal) => {
              if (!scope || !runtimeHostManager?.ownsScope(scope)) throw new Error('Preview target is unavailable');
              const target = runtimePolicyTargetsByEpoch.get(scope.targetEpoch);
              if (!target?.isActive()) throw new Error('Preview target is no longer active');
              return managedArtifactPreview.prepare(scope.targetEpoch, target.client, sessionId, artifactId, signal);
            }).map(tool => ({ context: 'session' as const, tool })),
          },
          {
            offerId: "desktop_settings",
            label: "Client settings",
            description:
              "Read or update UI and operating-system settings owned by this Desktop client.",
            tools: clientSettingsTools,
          },
          {
            offerId: "desktop_rive",
            label: "Rive",
            description:
              "Use durable Rive workflows through this Desktop client.",
            tools: [riveWorkflowTool],
          },
          // One offer per MCP server keeps grant contracts server-scoped: a
          // server change re-prompts only that server's tools.
          ...[...mcpServers.keys()].sort().map((serverId) => ({
            offerId: `desktop_mcp_${clientCapabilityEntityId(serverId, 116)}`,
            label: `MCP: ${serverId}`.slice(0, 128),
            description:
              "Use MCP tools connected by this Desktop client.",
            tools: (mcpServers.get(serverId) ?? []).map((identified) => ({
              tool: identified.tool,
              serverId: identified.serverId,
              toolName: identified.toolName,
            })),
            dynamic: true as const,
          })),
        ];
      },
      additionalServices: () => [
        pluginNotifications({
          local(packageId, title, body) {
            if (isE2e) return;
            if (!Notification.isSupported()) throw new Error('Native notifications are unavailable');
            const notification = new Notification({ title, body: `${packageId}\n${body}` });
            notification.on('click', () => mainWindowController.focus());
            notification.show();
          },
          async channel(channel, recipient, title, body) {
            if (!isBotDeliveryProvider(channel)) throw new Error('Notification channel is unavailable');
            if (!await botRegistry.sendMessage(channel, recipient, [title, body].filter(Boolean).join('\n\n')))
              throw new Error('Notification delivery failed');
          },
        }),
      ],
      oauthPresentation,
      releaseDesktopInteractionSession,
    },
    botRegistry,
    resolveBotCreateTarget: async (target) => ({
      workspace: await currentDesktopWorkspaceTarget(target),
    }),
    resolveSessionCreateProject: async (input, target) => {
      const currentTarget = requireRuntimePolicyTarget(target);
      return resolveDesktopSessionWorkspace(
        input,
        {
          ...currentTarget.projectManagement,
          ...(!runtimeHostProfileUsesHostWorkspace(target.kind)
            ? {
                defaultProjectId: async () =>
                  (await settingsStore.get()).projects.defaultProjectId,
              }
            : {}),
        },
        currentTarget.projectCatalog,
        { allowHostPath: !runtimeHostProfileUsesHostWorkspace(target.kind) },
      );
    },
    emitSessionsChanged,
    cacheTranscript: (scope, snapshot) => sessionLocal.cacheTranscript(scope, snapshot),
    ...(e2eFixture?.scenario === "chat-partial-history"
      ? { transcriptHistoryBytes: PARTIAL_HISTORY_TRANSCRIPT_BYTES }
      : {}),
    completeDesktopInteractionTurn,
    createSessionCopyCleanup: ({ removeSession, resumeSessionCopy }) =>
      createSessionCopyCleanupAuthority({
        workspaceRoot: desktopControlRoot,
        removeSession,
        resumeSessionCopy,
        processId: sessionCopyOwnerProcessId,
        databaseOptions: desktopDatabaseOptions,
      }),
    renderer: mainWindowController,
    onError: (error) =>
      console.error("[runtime-host] projection refresh failed:", error),
    registerClientIpc: registerHostClientIpc,
    openSshTunnel: runtimeHostSshTerminal.openSshTunnel,
    activateSshOperator: runtimeHostSshTerminal.activateSshOperator,
    resolveLocalCollaborationConnectionTarget: () =>
      localRuntimeHostRemoteAccess.createCollaborationConnectionTarget(),
    resolveProfileCollaborationConnectionTarget: (profile) =>
      runtimeHostProfileService.resolveCollaborationConnectionTarget(profile),
  },
  {
    background: true,
    handoffSurface: createDesktopHostHandoffSurface(() => desktopLocale.resolve()),
    onTargetStateChanged: (state) => {
      if (state.readiness === 'ready') console.info('[startup-metric]', {
        hostReadyMs: Math.round(process.uptime() * 1000), profileId: state.target.profile.id,
      });
      const localTarget = localSessionTarget(state);
      if (localTarget) {
        sessionLocalStore.bindAuthority(localTarget.profileId, localTarget.partition);
        if (state.readiness === 'unavailable' && state.error instanceof RuntimeHostProfileConnectionError && state.error.reason === 'credential_rejected') sessionLocal.purge(localTarget);
      }
      sessionLocal.wake();
      const profileAccess = runtimeHostProfileAccess(state.target.profile);
      const hostId = state.readiness === "ready"
        ? state.candidate.client.hostId
        : state.hostId;
      mainWindowController.send("runtime-host-profiles:changed", {
        epoch: state.epoch,
        profileId: state.target.profile.id,
        profileName: state.target.profile.name,
        profileKind: state.target.profile.kind,
        profileAccess,
        ...(hostId ? { hostId } : {}),
        readiness: state.readiness,
        isDefault:
          (runtimeHostManager?.defaultProfileId() ??
            runtimeHostStartup.preferences.defaultProfileId) === state.target.profile.id,
      });
      if (profileAccess === 'session_guest') {
        void guestSessionMountService
          .connectionChanged(
            state.target.profile.id,
            state.readiness === 'unavailable' ? state.error : undefined,
          )
          .catch((error: unknown) =>
            console.warn('[runtime-host] shared Session connection update failed:', error),
          );
      }
      if (state.readiness === "unavailable" && state.hostId) {
        void browserIpc.retireTarget({
          hostId: state.hostId,
          targetEpoch: state.epoch,
        }).catch((error) =>
          console.error("[runtime-host] Browser target retirement failed:", error),
        );
      }
      if (
        state.readiness === 'unavailable' &&
        state.target.profile.id === runtimeHostManager?.defaultProfileId()
      ) {
        defaultRuntimeHostRecovery.offer({
          profileId: state.target.profile.id,
          profileName: state.target.profile.name,
          error: state.error,
        });
      }
      if (state.readiness === "ready") {
        const scope = { hostId: state.candidate.client.hostId, targetEpoch: state.epoch };
        mainWindowController.send("projects:changed", scope);
        emitConnectionListChanged(scope);
      }
    },
    onTargetRemoved: (state) => {
      const localTarget = localSessionTarget(state);
      if (localTarget) sessionLocal.purge(localTarget);
      const hostId = state.readiness === "ready"
        ? state.candidate.client.hostId
        : state.hostId;
      mainWindowController.send("runtime-host-profiles:changed", {
        epoch: state.epoch,
        profileId: state.target.profile.id,
        profileName: state.target.profile.name,
        profileKind: state.target.profile.kind,
        profileAccess: runtimeHostProfileAccess(state.target.profile),
        ...(hostId ? { hostId } : {}),
        readiness: "unavailable",
        isDefault:
          (runtimeHostManager?.defaultProfileId() ??
            runtimeHostStartup.preferences.defaultProfileId) === state.target.profile.id,
        removed: true,
      });
      const scope = hostId ? { hostId, targetEpoch: state.epoch } : undefined;
      if (scope) {
        void browserIpc.retireTarget(scope).catch((error) =>
          console.error("[runtime-host] Browser target retirement failed:", error),
        );
      }
    },
    onDefaultProfileChanged: (profileId) => {
      const state = runtimeHostManager?.entries().find(
        (candidate) => candidate.target.profile.id === profileId,
      );
      mainWindowController.send("runtime-host-profiles:changed", {
        epoch: state?.epoch ?? randomUUID(),
        profileId,
        profileName: state?.target.profile.name ?? profileId,
        profileKind: state?.target.profile.kind ?? "remote",
        profileAccess: state ? runtimeHostProfileAccess(state.target.profile) : "owner",
        ...(state?.readiness === "ready"
          ? { hostId: state.candidate.client.hostId }
          : state?.readiness !== "unavailable" && state && "hostId" in state && state.hostId
            ? { hostId: state.hostId }
            : {}),
        readiness: state?.readiness ?? "unavailable",
        isDefault: true,
      });
    },
    recoverLocalHost: (signal) => isE2e
      ? localRuntimeHostRemoteAccess.recoverBeforeLocalHostStart(signal)
      : Promise.resolve(false),
    resolveStartupRepair: (error, signal) => isE2e
      ? localRuntimeHostRemoteAccess.resolveStartupRepair(error, signal)
      : Promise.resolve(undefined),
    resolveWslHostHandoff: async (profile, error, signal) => resolveDesktopWslHostHandoff(profile, error, signal, {
      locale: await desktopLocale.resolve(),
      resolveBinding: (profileId) => runtimeHostProfileService.resolveManagedService(profileId),
      resolvePackage: (packageSignal) => runtimeHostSetupPackage.resolve('none', packageSignal),
    }),
    resolveLocalHostReplacement: (registration, signal) =>
      isE2e
        ? localRuntimeHostRemoteAccess.resolveConflictingHostReplacement(registration, signal)
        : Promise.resolve(undefined),
    onFatalError: (error, target) => {
      // Host availability never owns the Desktop lifetime.
      console.error("[runtime-host] unavailable:", target.profile.id, error);
    },
  },
);
let workBoardIpc: ReturnType<typeof registerWorkBoardIpc> | undefined;
let runtimeHostDesktopShutdown: Promise<void> | undefined;
// The first Host handoff can be cancelled before the main window exists.
// Install the same cleanup owner used by normal quit before that handoff.
const quitCoordinator = createAppQuitCoordinator({
  prepareToQuit: prepareRuntimeHostDesktopQuit,
  cleanup: closeRuntimeHostDesktop,
  focusOrCreateWindow: (signal) => {
    if (!runtimeHostManager) return;
    if (mainWindowController.hasOpenWindows()) mainWindowController.focus();
    else return mainWindowController.createWindow(signal);
  },
  onPreparationError: (error) => {
    console.error("[runtime-host] quit retirement failed:", error);
  },
  onCleanupError: (error) =>
    console.error("[runtime-host] shutdown failed:", error),
  onWindowCreationError: (error) =>
    console.error("[window] creation failed:", error),
  resumeQuit: () => app.quit(),
});
app.on("before-quit", quitCoordinator.handleBeforeQuit);
updateDesktopStartupProgress('connect');
runtimeHostManager = await startLocalRuntimeHostManager();
// Desktop owns these tables in its own directory. The TS E2E composition retains
// its existing shared-database migration authority.
workBoardIpc = registerWorkBoardIpc({
  ipcMain,
  workspaceRoot,
  mainWindowController,
  store: createWorkBoardStore(desktopControlRoot, desktopDatabaseOptions),
  validateLinkedSession: async (value, expectedProjectId) => {
    const normalized = normalizeWorkBoardLinkedSession(value);
    if (!normalized.ok) return false;
    try {
      const current = runtimeHostManager?.current(normalized.value.profileId);
      if (!current?.candidate || current.hostId !== normalized.value.hostId) return false;
      const sessions = await current.candidate.client.listSessions();
      const session = sessions.find((candidate) => candidate.id === normalized.value.sessionId);
      if (!session) return false;
      if (expectedProjectId !== undefined) {
        return (
          session.workspace.target.kind === 'project' &&
          session.workspace.target.projectId === expectedProjectId
        );
      }
      return true;
    } catch {
      return false;
    }
  },
});
updateDesktopStartupProgress('renderer');
wireLifecycle();
runtimeHostManager.setDefaultProfile(runtimeHostStartup.preferences.defaultProfileId);
sessionLocal.wake();
windowsAppTray.start();
await guestSessionMountService.start().catch((error: unknown) => {
  console.error('[runtime-host] shared Sessions could not be restored:', error);
});
if (isE2e) {
  await localRuntimeHostRemoteAccess.recover().catch((error: unknown) => {
    console.error('[runtime-host] interrupted Local Host setup could not be recovered:', error);
  });
}
void runtimeHostProfileService.startEnabledProfiles();
const unavailableDefault = runtimeHostStartup.unavailable.get(
  runtimeHostStartup.preferences.defaultProfileId,
);
if (unavailableDefault) {
  void runtimeHostProfileService
    .getSnapshot()
    .then((snapshot) => {
      const entry = snapshot.entries.find((candidate) => candidate.isDefault);
      defaultRuntimeHostRecovery.offer({
        profileId: runtimeHostStartup.preferences.defaultProfileId,
        profileName:
          entry?.profile.name ?? runtimeHostStartup.preferences.defaultProfileId,
        error: unavailableDefault,
      });
    })
    .catch((error) =>
      console.error("[runtime-host] failed to resolve unavailable default Host:", error),
    );
}
const stopComputerUseSession = (sessionId: string): void => {
  const ref = parseDesktopSessionResourceKey(sessionId);
  void runtimeHostManager
    ?.stopSession(ref)
    .catch((error) => console.error("[runtime-host] stop failed:", error));
};
native.computerUsePip.setStopHandler(stopComputerUseSession);
native.computerUseStatusItem.setStopHandler(stopComputerUseSession);

updateService.start();
void ensureMcpReady()
  .then(() => mcpCapabilityPublisher.refreshIfChanged())
  .catch((error) => console.error("[runtime-host] MCP startup failed:", error));
// A login round persists its verifier and callback port; if the app
// restarted mid-round, rebind the listener so the browser's redirect still
// lands instead of hitting a dead port. Deliberately NOT chained behind the
// connect/publish sequence above: a slow server or a publish failure must
// not delay or block the rebind — it needs only the persisted state, and
// the controller awaits readiness itself before the token exchange.
void mcpConfigStore
  .get()
  .then((config) => {
    for (const serverId of Object.keys(config.mcpServers)) {
      void mcpOAuthController
        .resumeLogin(serverId)
        // No explicit mcp:changed here: a successful resume ends in
        // finishAuthorization → reconnect, whose onChange handler already
        // emits AND refreshes capabilities — a second identical emit here
        // was strictly weaker.
        .catch((error) =>
          console.error(
            `[runtime-host] MCP login resume failed for ${serverId}:`,
            error,
          ),
        );
    }
  })
  .catch((error) =>
    console.error("[runtime-host] MCP login resume scan failed:", error),
  );

void clientSettingsEffects
  .refresh(false)
  .catch((error) =>
    console.error("[runtime-host] Client settings startup failed:", error),
  );

function registerHostClientIpc(
  client: DesktopRuntimeHostClient,
  scopedIpc: ReconnectableReadIpcMain,
  controls: DesktopRuntimeHostCandidateControls,
  target: DesktopRuntimeHostTargetPolicy,
  scope: DesktopTargetScope,
  isTargetActive: () => boolean,
): () => Promise<void> {
  const usesHostWorkspace = runtimeHostProfileUsesHostWorkspace(target.kind);
  const sendToRenderer = (channel: string, ...args: unknown[]): void => {
    if (isTargetActive()) mainWindowController.send(channel, scope, ...args);
  };
  const emitTargetConnectionListChanged = (): void => {
    if (isTargetActive()) emitConnectionListChanged(scope);
  };
  const emitTargetSessionsChanged = (
    reason: SessionChangedReason,
    sessionId?: string,
    extra?: Pick<SessionChangedEvent, "modelId" | "turnId">,
  ): void => {
    if (isTargetActive()) emitSessionsChanged(scope, reason, sessionId, extra);
  };
  const targetProjectRoot = createProjectRootController({
    rootId: target.rootId,
    preferenceFile: join(workspaceRoot, "project-preferences.json"),
    fallbackRoots: () => [process.cwd(), app.getAppPath()],
  });
  const targetProjectCatalog = createRuntimeHostProjectCatalog(() => ({
    client,
    includeHostPaths: !usesHostWorkspace,
  }));
  const targetProjectManagement = createProjectManagementService({
    catalog: targetProjectCatalog,
    directoryCatalog: targetProjectCatalog,
    chooseDirectory: async () => {
      const result = await mainWindowController.showOpenDialog({
        title: projectPickerTitle(await desktopLocale.resolve()),
        properties: ["openDirectory"],
      });
      return result.canceled ? undefined : result.filePaths[0];
    },
    selection: targetProjectRoot,
    capabilities: !usesHostWorkspace
      ? {
          chooseClientDirectory: true,
          chooseHostDirectory: false,
          selectNoProject: true,
          setLocalDefault: true,
          viewClientPath: true,
        }
      : {
          chooseClientDirectory: false,
          chooseHostDirectory: true,
          selectNoProject: false,
          setLocalDefault: false,
          viewClientPath: false,
        },
  });
  const targetContext = {
    client,
    policy: target,
    scope,
    projectCatalog: targetProjectCatalog,
    projectManagement: targetProjectManagement,
    isActive: isTargetActive,
  };
  runtimePolicyTargets.set(target, targetContext);
  runtimePolicyTargetsByEpoch.set(scope.targetEpoch, targetContext);
  const unsubscribeConfigurationChanges = client.subscribeConfigurationChanges(() => {
    emitTargetConnectionListChanged();
    sendToRenderer("settings:externalChanged", { ts: Date.now() });
  });
  // No `settings:externalChanged` here: the user's settings did not move, the
  // Host just resolved the same connections against a newer model catalog.
  const unsubscribeConnectionCatalogChanges = client.subscribeConnectionCatalogChanges(() => {
    emitTargetConnectionListChanged();
  });
  const unsubscribeSessionCatalogChanges = client.subscribeSessionCatalogChanges(
    ({ sessionId }) => emitTargetSessionsChanged("updated", sessionId),
  );
  const unsubscribeProjectCatalogChanges = client.subscribeProjectCatalogChanges(() => {
    sendToRenderer("projects:changed");
  });
  const unsubscribePluginClientChanges = client.subscribePluginClientChanges((revision) => {
    sendToRenderer("plugins:client-changed", revision);
  });
  const capabilityBinding = mcpCapabilityPublisher.bind(
    controls.refreshClientCapabilities,
  );
  void capabilityBinding.aligned.catch((error) =>
    console.error("[runtime-host] MCP capability alignment failed:", error),
  );
  registerMcpIpcMain({
    ipcMain: scopedIpc,
    store: mcpConfigStore,
    manager: mcpManager,
    oauth: mcpOAuthController,
    exclusiveLane: mcpExclusiveLane,
    ensureReady: ensureMcpReady,
    publishCapabilities: mcpCapabilityPublisher.refreshIfChanged,
    onPublicationError: (error) =>
      console.error("[runtime-host] MCP capability publication failed:", error),
    emitChanged: (statuses) =>
      sendToRenderer("mcp:changed", statuses),
  });
  registerRuntimeHostConnectionsIpc({
    ipcMain: scopedIpc,
    client,
    emitConnectionListChanged: emitTargetConnectionListChanged,
  });
  registerRuntimeHostRendererIpc({ ipcMain: scopedIpc, client });
  const disposeClientPluginRemotes = registerClientPluginRemoteIpc({
    ipcMain: scopedIpc, client,
    ownsRenderer: (contents) => mainWindowController.ownsRenderer(contents),
    report: (error) => console.error('[plugins] Remote cleanup failed:', error),
    authorization: {
      validate: async (identity) => { await client.request('plugin.client.query', {kind:'bundle',entryId:identity.entryId,activation:identity.activation,clientDigest:identity.clientDigest,offset:0}); },
      confirm: async (input, signal) => {
        const locale = desktopLocale.current();
        const result = await showDesktopMessageBox(pluginAuthorizationDialog(input, locale, signal), {locale});
        return result.response === 1;
      },
      request: (input) => client.request('plugin.authorization', input),
    },
    files: usesHostWorkspace ? undefined : {
      validate: async (input) => { await client.request('plugin.client.query', input); },
      pick: async () => {
        const selection = await mainWindowController.showOpenDialog({properties:['openFile']});
        return selection.canceled ? null : selection.filePaths[0] ?? null;
      },
      open: async (path) => {
        const error = await shell.openPath(path);
        if (error) throw new Error(error);
      },
    },
  });
  registerRuntimeHostArtifactsIpc({
    uiLocale: () => desktopLocale.current(),
    ipcMain: scopedIpc,
    client,
    mainWindowController,
    showItemInFolder: (path) => shell.showItemInFolder(path),
    openPath: (path) => shell.openPath(path),
    preview: { service: managedArtifactPreview, scope: scope.targetEpoch, openExternal: (url) => shell.openExternal(url) },
  });
  registerExternalAgentSetupIpc({ ipcMain: scopedIpc, client, presentation: oauthPresentation,
    selectExecutable: async () => {
      const result = await mainWindowController.showOpenDialog({ properties: ['openFile'] });
      return result.canceled ? undefined : result.filePaths[0];
    },
  });
  registerRuntimeHostOAuthIpc({
    ipcMain: scopedIpc,
    client,
    presentation: oauthPresentation,
    emitConnectionListChanged: emitTargetConnectionListChanged,
  });
  registerRuntimeHostGitHubCopilotIpc({
    ipcMain: scopedIpc,
    client,
    emitConnectionListChanged: emitTargetConnectionListChanged,
  });
  registerRuntimeHostMemoryIpc({
    ipcMain: scopedIpc,
    client,
    workspaceRoot,
    openPath: (path) => shell.openPath(path),
    allowLocalPaths: !usesHostWorkspace,
  });
  const runtimeHostSettings = createRuntimeHostSettingsModule({
    client,
    settingsStore,
    applyClientSettings: async (settings) => {
      await clientSettingsEffects.apply(settings, true);
    },
  });
  registerRuntimeHostSettingsIpc({
    ipcMain: scopedIpc,
    module: runtimeHostSettings,
  });
  registerRuntimeHostConfigIpc({
    uiLocale: () => desktopLocale.current(),
    ipcMain: scopedIpc,
    client,
    mainWindowController,
    appVersion: app.getVersion(),
    settingsModule: runtimeHostSettings,
    emitConnectionsChanged: emitTargetConnectionListChanged,
  });
  registerRuntimeHostPermissionsIpc({
    ipcMain: scopedIpc,
    client,
    sandboxSetup: {
      isCurrent: () => runtimePolicyTargetsByEpoch.get(scope.targetEpoch) === targetContext,
      confirm: async () => {
        const locale = desktopLocale.current();
        const result = await showDesktopMessageBox(sandboxSetupDialog(locale), {locale});
        return result.response === 1;
      },
      unavailable: (status) => sandboxSetupUnavailable(desktopLocale.current(), status),
    },
    getSettings: () => runtimeHostSettings.get(),
    listConnections: async () =>
      projectHostConnections(await client.loadConnectionCatalog()),
    botRegistry,
    getComputerUseCapabilityInput: () => {
      const executorState = native.computerUse.backend?.executorState?.();
      return {
        backendId: native.computerUse.backendId,
        health: computerUseServiceHealth(
          native.computerUse.backendId,
          executorState,
        ),
      };
    },
  });
  registerPermissionOverlayIpc({
    controller: permissionOverlay,
    ipcMain: scopedIpc,
  });
  registerRuntimeHostSearchIpc({ ipcMain: scopedIpc, client });
  registerRuntimeHostUsageIpc({
    ipcMain: scopedIpc,
    client,
    sendToRenderer,
  });
  registerRuntimeHostWorkspaceIpc({
    ipcMain: scopedIpc,
    client,
    allowLocalWorkspace: !usesHostWorkspace,
  });
  const resolveProjectRootForContext = (sessionId: unknown): Promise<string> =>
    resolveProjectContextRoot(sessionId, {
      currentProjectRoot: () => targetProjectRoot.current(),
      readSessionCwd: async (id) => {
        const session = await client.getSession(id);
        if (!session) throw new Error(`No such Session: ${id}`);
        return session.workspace.hostCwd;
      },
    });
  registerAppIpc(
    {
      projectRoot: targetProjectRoot,
      getSessionProjectRoot: (sessionId) =>
        resolveProjectRootForContext(sessionId),
      getProjectRoot: resolveProjectRootForContext,
      workspaceRoot,
      buildInfo,
      updateChannel: desktopUpdateChannel,
      e2eFixture,
      projectManagement: targetProjectManagement,
      allowLocalProjectPaths: !usesHostWorkspace,
    },
    scopedIpc,
  );
  registerWorkspaceSearchIpc({
    ipcMain: scopedIpc,
    getProjectRoot: async (sessionId, projectId) => {
      if (typeof projectId !== "string") {
        return resolveProjectRootForContext(sessionId);
      }
      const path = await targetProjectManagement.pathFor(projectId);
      if (!path) throw new Error(`Project is unavailable: ${projectId}`);
      return path;
    },
    allowLocalWorkspace: !usesHostWorkspace,
  });
  const onboardingService = createOnboardingService({
    listConnections: async () =>
      projectHostConnections(await client.loadConnectionCatalog()),
    getDefaultSlug: async () => {
      const catalog = await client.loadConnectionCatalog();
      const target = catalog.defaultTarget;
      return target === null
        ? null
        : (catalog.connections.find(
            ({ connectionId }) => connectionId === target.connectionId,
          )?.slug ?? null);
    },
    listSessions: async () =>
      (await client.listSessions()).map(toDesktopHostSessionSummary),
    getMilestones: async () =>
      (await settingsStore.get()).onboarding.milestones,
    upsertMilestone: (id, status) =>
      settingsStore.upsertOnboardingMilestone(id, status),
    hasCredential: (connection) =>
      readWithFallback(async () => {
        if (!providerAuthRequiresSecret(connection.providerType)) return true;
        const catalog = await client.loadConnectionCatalog();
        const entry = catalog.connections.find(
          ({ slug }) => slug === connection.slug,
        );
        if (!entry) return false;
        const authKind = PROVIDER_REGISTRY[entry.providerType].authKind;
        const status = await client.queryCredential({
          scope: "connection",
          connectionId: entry.connectionId,
          kind: authKind === "oauth_token" ? "oauth_token" : "api_key",
        });
        return status?.configured === true;
      }, false),
  });
  const taskSubmissionReadinessService = createDesktopTaskSubmissionReadinessService({
    workspaceRoot,
    runtimeState: () => ({ state: client.lifecycleState, checkedAt: Date.now() }),
    ...(usesHostWorkspace
      ? { inspectWorkspace: async () => "ready" as const }
      : {}),
    resolveModelTarget: (requestedSlug) =>
      readWithFallback<DesktopModelTargetResolution>(async () => {
        const catalog = await client.loadConnectionCatalog();
        const connections = projectHostConnections(catalog);
        const connectionSlug = requestedSlug ?? (catalog.defaultTarget === null
          ? undefined
          : catalog.connections.find(
              ({ connectionId }) => connectionId === catalog.defaultTarget?.connectionId,
            )?.slug);
        if (!connectionSlug) return { kind: "missing_default" } as const;
        const connection = connections.find(({ slug }) => slug === connectionSlug);
        if (!connection) return { kind: "connection_missing", connectionSlug } as const;
        if (!providerAuthRequiresSecret(connection.providerType)) {
          return { kind: "resolved", connection, hasSecret: true } as const;
        }
        const entry = catalog.connections.find(({ slug }) => slug === connection.slug);
        if (!entry) return { kind: "connection_missing", connectionSlug } as const;
        const authKind = PROVIDER_REGISTRY[entry.providerType].authKind;
        const hasSecret = await client.queryCredential({
          scope: "connection",
          connectionId: entry.connectionId,
          kind: authKind === "oauth_token" ? "oauth_token" : "api_key",
        }).then((status) => status?.configured === true);
        return { kind: "resolved", connection, hasSecret } as const;
      }, { kind: "unknown" }),
  });
  registerOnboardingIpc({ onboardingService, ipcMain: scopedIpc });
  registerTaskSubmissionReadinessIpc(taskSubmissionReadinessService, scopedIpc);
  return async () => {
    unsubscribeConfigurationChanges();
    await disposeClientPluginRemotes();
    await managedArtifactPreview.closeScope(scope.targetEpoch);
    unsubscribeConnectionCatalogChanges();
    unsubscribeSessionCatalogChanges();
    unsubscribeProjectCatalogChanges();
    unsubscribePluginClientChanges();
    runtimePolicyTargets.delete(target);
    if (runtimePolicyTargetsByEpoch.get(scope.targetEpoch) === targetContext) {
      runtimePolicyTargetsByEpoch.delete(scope.targetEpoch);
    }
    capabilityBinding.dispose();
    await capabilityBinding.aligned.catch(() => undefined);
  };
}


function registerPersistentClientIpc(): void {
  registerAppClientIpc({
    mainWindowController,
    e2eFixture,
    updateService,
  });
  registerAppIconIpc({
    ipcMain,
    showOpenDialog: (options) => mainWindowController.showOpenDialog(options),
    listPreviews: () => listAppIconPreviews(),
    importArtwork: (source) => importCustomAppIcon(source),
    userDataPath: () => app.getPath('userData'),
    settingsStore,
    applySettings: async (settings) => {
      await clientSettingsEffects.apply(settings, true);
    },
  });
  registerMarkdownSaveIpc({
    ipcMain,
    mainWindowController,
    resolveLocale: () => desktopLocale.resolve(),
  });
  registerCommandCodeLoginIpc({ ipcMain, controller: commandCodeLoginController });
  registerDesktopRuntimeHostProfileIpc(ipcMain, runtimeHostProfileService);
  registerDesktopGuestSessionMountIpc(
    ipcMain,
    guestSessionMountService,
    () => clipboard.readText(),
  );
  registerClientSettingsIpc({
    ipcMain,
    settingsStore,
    apply: async (settings) => {
      await clientSettingsEffects.apply(settings, true);
    },
  });
  settingsBotsIpc = registerSettingsBotsIpc({
    ipcMain,
    settingsStore,
    botRegistry,
    applySettingsRuntimeEffects: async (settings) => {
      await clientSettingsEffects.apply(settings, true);
    },
    productVersion: app.getVersion(),
    openExternal: (url) => shell.openExternal(url),
    ...(useBotOnboardingFixture
      ? {
          botOnboardingAdapters: createE2eFixtureBotOnboardingAdapters(),
          botOnboardingReadChannelStatus: () => ({ running: true }),
        }
      : {}),
  });
  ipcMain.handle("sessions:unobserve", async (_event, observerId: unknown) => {
    if (typeof observerId !== "string" || observerId.length === 0 || observerId.length > 256) {
      throw new Error("Invalid Session observer identity");
    }
    await runtimeHostManager?.unobserveSession(observerId);
  });
  ipcMain.handle('sessions:transcript:close', async (event, consumerId: unknown) => {
    if (typeof consumerId !== 'string' || consumerId.length === 0 || consumerId.length > 256) {
      throw new Error('Invalid transcript consumer identity');
    }
    await runtimeHostManager?.closeTranscript(consumerId, event.sender.id);
  });
  ipcMain.handle(
    'sessions:transcript:ack',
    (event, scope: unknown, consumerId: unknown, generation: unknown, deliverySequence: unknown) => {
      const target = requireDesktopTargetScope(scope);
      if (typeof consumerId !== 'string' || consumerId.length === 0 || consumerId.length > 256) {
        throw new Error('Invalid transcript consumer identity');
      }
      if (typeof generation !== 'string' || generation.length === 0 || generation.length > 256) {
        throw new Error('Invalid transcript generation');
      }
      if (!Number.isSafeInteger(deliverySequence) || Number(deliverySequence) < 0) {
        throw new Error('Invalid transcript delivery');
      }
      runtimeHostManager?.acknowledgeTranscript(
        target,
        consumerId,
        generation,
        Number(deliverySequence),
        event.sender.id,
      );
    },
  );
  const projectRuntimeHostIdentity = (
    epoch: string,
    target: ResolvedRuntimeHostProfile,
    readiness: 'ready' | 'reconnecting',
    hostId: string,
  ): DesktopRuntimeHostIdentity => ({
    hostId,
    targetEpoch: epoch,
    profileId: target.profile.id,
    profileName: target.profile.name,
    profileKind: target.profile.kind,
    profileAccess: runtimeHostProfileAccess(target.profile),
    readiness,
  });
  ipcMain.handle("runtime-host:activeIdentity", () => {
    const current = runtimeHostManager?.current();
    if (!current?.hostId) {
      throw new Error("Desktop Runtime Host identity is unavailable");
    }
    return projectRuntimeHostIdentity(
      current.epoch,
      current.target,
      current.readiness,
      current.hostId,
    );
  });
  ipcMain.handle("runtime-host:identities", () =>
    (runtimeHostManager?.entries() ?? []).flatMap((state) => {
      if (state.readiness === 'unavailable' && state.error instanceof RuntimeHostProfileConnectionError && state.error.reason === 'credential_rejected') return [];
      const hostId = state.readiness === "ready" ? state.candidate.client.hostId : state.hostId ?? localSessionTarget(state)?.scope.hostId;
      if (!hostId) return [];
      return [
        projectRuntimeHostIdentity(state.epoch, state.target, state.readiness === 'ready' ? 'ready' : 'reconnecting', hostId),
      ];
    }),
  );
  registerDesktopDiagnosticsIpc({ ipcMain, ...desktopDiagnostics });
  ipcMain.handle('directories:pick', async () => {
    const local = runtimeHostManager?.entries().find(
      (state) => state.target.profile.kind === 'local',
    );
    if (!local || local.readiness !== 'ready') throw new Error('Local Runtime Host is unavailable');
    const hostId = local.candidate.client.hostId;
    const result = await mainWindowController.showOpenDialog({
      title: nativeFileDialogCopy(await desktopLocale.resolve()).referenceFolder,
      properties: ['openDirectory'],
    });
    if (result.canceled || !result.filePaths[0]) return { ok: false, reason: 'cancelled' };
    return { ok: true, reference: { hostId, path: result.filePaths[0] } };
  });
  ipcMain.handle("attachments:pickFiles", async (event) => {
    const result = await mainWindowController.showOpenDialog({
      title: nativeFileDialogCopy(await desktopLocale.resolve()).addAttachments,
      properties: ["openFile", "multiSelections"],
    });
    if (result.canceled || !result.filePaths[0])
      return { ok: false, reason: "cancelled" };
    const { stat } = await import("node:fs/promises");
    // Route by content, not the extension: each picked path is staged under the
    // kind its sniffed MIME implies, so a real image named `report.pdf` previews
    // and triggers the vision notice, and a disguised file does neither.
    const chosen = await resolvePickedAttachments(result.filePaths, (path) => stat(path));
    return {
      ok: true,
      files: attachmentApprovals.issueApprovals(event.sender.id, chosen),
    };
  });
  registerAttachmentPreviewIpc({
    ipcMain,
    approvals: attachmentApprovals,
    readFile: readFileCapped,
    renderPreview: renderAttachmentPreview,
  });
}

function requireRuntimePolicyTarget(target: DesktopRuntimeHostTargetPolicy) {
  const current = runtimePolicyTargets.get(target);
  if (!current || !current.isActive()) {
    throw new Error("Runtime Host target generation is no longer active");
  }
  return current;
}

function resolveRuntimeHostDiagnostics(scope: DesktopTargetScope) {
  if (!runtimeHostManager?.ownsScope(scope)) {
    throw new Error("Desktop Runtime Host request belongs to a different target");
  }
  const current = runtimePolicyTargetsByEpoch.get(scope.targetEpoch);
  if (!current || !current.isActive()) return undefined;
  const client = current.client;
  return {
    getDiagnostics: () => client.queryHostDiagnostics(),
    getTurnTrace: async (sessionId: string, turnId: string, timeoutMs: number) => {
      const result = await client.request('execution.inspect.query', {
        kind: "turn_trace",
        sessionId,
        turnId,
      }, timeoutMs);
      return result.kind === "turn_trace" ? result.turn : undefined;
    },
  };
}

function sendActiveRuntimeHostEvent(channel: string, ...args: unknown[]): void {
  const scope = activeRuntimeHostRef();
  if (scope) mainWindowController.send(channel, scope, ...args);
}

function emitConnectionListChanged(scope: DesktopTargetScope): void {
  const event: ConnectionEvent = {
    type: "connection_list_changed",
    id: randomUUID(),
    ts: Date.now(),
  };
  mainWindowController.send("connections:event", scope, event);
}

function emitSessionsChanged(
  scope: DesktopTargetScope,
  reason: SessionChangedReason,
  sessionId?: string,
  extra?: Pick<SessionChangedEvent, "modelId" | "turnId">,
): void {
  sessionLocal.changed(scope);
  if (reason === 'deleted' && sessionId) {
    try { sessionLocalStore.removeSession(sessionLocal.target(scope).partition, sessionId); } catch { /* A retired target cannot repopulate its cache. */ }
  }
  const event: SessionChangedEvent = {
    reason,
    ts: Date.now(),
    ...(sessionId ? { sessionId } : {}),
    ...(extra?.modelId ? { modelId: extra.modelId } : {}),
    ...(extra?.turnId ? { turnId: extra.turnId } : {}),
  };
  mainWindowController.send("sessions:changed", scope, event);
}

function wireLifecycle(): void {
  installDesktopShellPresentation({
    mainWindowController,
    focusOrCreateWindow: quitCoordinator.focusOrCreateWindow,
  });
  app.on("second-instance", quitCoordinator.focusOrCreateWindow);
  app.on("activate", quitCoordinator.focusOrCreateWindow);
  app.on("browser-window-focus", () => {
    void updateService.checkForUpdatesOnFocus();
  });
  app.on("window-all-closed", () => {
    native.computerUseOverlay.destroyAll();
    native.computerUsePip.destroyAll();
    if (process.platform !== "darwin" && !windowsAppTray.hasTray() && !isBrowserMessageBoxPresentationActive() &&
      !isDesktopStartupInProgress()) app.quit();
  });
  powerMonitor.on("resume", wakePeerRecoveryAfterResume);
  quitCoordinator.focusOrCreateWindow();
}

async function prepareRuntimeHostDesktopQuit(signal?: AbortSignal): Promise<'ready' | 'cancelled'> {
  const preparation = await prepareRuntimeHostQuit(runtimeHostManager, {
    confirmInterrupt: async () => {
      const locale = await desktopLocale.resolve();
      const dialog = buildRuntimeHostActiveQuitDialog(locale);
      const { response } = await showDesktopMessageBox(dialog.options, { locale });
      return dialog.decisions[response] === 'quit';
    },
  }, signal);
  if (preparation === 'ready') mainWindowController.browserWindow()?.destroy();
  return preparation;
}

function closeRuntimeHostDesktop(): Promise<void> {
  return runtimeHostDesktopShutdown ??= disposeRuntimeHostDesktop();
}

async function disposeRuntimeHostDesktop(): Promise<void> {
  ipcMain.removeHandler('runtime-host-management:native');
  // Any in-flight browser sign-in ends here with the app; its loopback port goes with it.
  commandCodeLoginController.dispose();
  sessionLocal.close();
  powerMonitor.off("resume", wakePeerRecoveryAfterResume);
  clientSettingsWatcher.stop();
  updateService.dispose();
  settingsBotsIpc?.dispose();
  permissionOverlay.dismiss();
  const guestMountShutdown = Promise.resolve().then(() => guestSessionMountService.close());
  const runtimeHostManagerShutdown = guestMountShutdown
    .catch(() => undefined)
    .then(() => runtimeHostManager?.close());
  const runtimeHostPeerShutdown = runtimeHostManagerShutdown
    .catch(() => undefined)
    .then(async () => {
      const errors: unknown[] = [];
      await runtimeHostPeerMeshComponent?.close().catch((error: unknown) => errors.push(error));
      await runtimeHostPeerEndpointOwner?.close().catch((error: unknown) => errors.push(error));
      if (errors.length === 1) throw errors[0];
      if (errors.length > 1) {
        throw new AggregateError(errors, 'Unable to close Desktop peer resources');
      }
    });
  const results = await Promise.allSettled([
    managedArtifactPreview.close(),
    Promise.resolve().then(() => windowsAppTray.dispose()),
    workHubControl.close(),
    Promise.resolve().then(() => workHubPresentation.dispose()),
    Promise.resolve().then(() => runtimeHostManagement.close()),
    Promise.resolve().then(() => runtimeHostPeerMeshManagement.close()),
    guestMountShutdown,
    runtimeHostManagerShutdown,
    runtimeHostPeerShutdown,
    runtimeHostOnboarding.close(),
    localRuntimeHostRemoteAccess.close(),
    runtimeHostSetupPackage.close(),
    Promise.resolve().then(() => workBoardIpc?.close()),
    runtimeHostSshTerminal.close(),
    botRegistry.stopAll(),
    mcpManager.close(),
    mainWindowController.disposeBrowserViews(),
    Promise.resolve().then(() => native.computerUseOverlay.destroyAll()),
    Promise.resolve().then(() => native.computerUsePip.destroyAll()),
    Promise.resolve().then(() => native.computerUseStatusItem.destroy()),
    Promise.resolve().then(() => native.computerUseScreenLock.dispose()),
    Promise.resolve().then(() => native.computerUse.backend?.dispose?.()),
  ]);
  for (const result of results) {
    if (result.status === "rejected")
      console.error("[runtime-host] shutdown failed:", result.reason);
  }
  sessionLocalStore.close();
}

function wakePeerRecoveryAfterResume(): void {
  runtimeHostManager?.notifySystemResume();
}

function resolveDesktopE2eFixture(): ReturnType<typeof resolveE2eFixture> {
  try {
    return resolveE2eFixture(
      process.env.MAKA_E2E_FIXTURE,
      app.isPackaged,
      process.env.MAKA_E2E_FIXTURE_REDUCED_MOTION,
      process.env.MAKA_E2E_FIXTURE_THEME,
      process.env.MAKA_E2E_FIXTURE_LOCALE,
      process.env.MAKA_E2E_FIXTURE_TIMEZONE,
      process.env.MAKA_E2E_FIXTURE_PLATFORM,
    );
  } catch (error) {
    if (!process.env.MAKA_E2E_FIXTURE) throw error;
    console.error(
      `[e2e-fixture] fatal: ${error instanceof Error ? error.message : String(error)}`,
    );
    process.exit(1);
  }
}

async function confirmDesktopStorageRootRepair(
  workspaceRoot: string,
): Promise<boolean> {
  console.log(
    "[storage-root] root-identity conflict; parking at repair dialog",
  );
  const locale = resolveSystemUiLocale(app.getPreferredSystemLanguages());
  const copy = getNativeDiagnosticDialogCopy(locale).storageRootRepair;
  const { response } = await showStartupDiagnosticDialog(
    {
      type: "warning",
      title: copy.title,
      message: copy.message,
      detail: copy.detail(workspaceRoot),
      buttons: [copy.repair, copy.exit],
      defaultId: 1,
      cancelId: 1,
      noLink: true,
    },
    locale,
  );
  return response === 0;
}

async function promptForDefaultRuntimeHostRecovery(input: {
  readonly profileName: string;
  readonly error: Error;
}): Promise<"retry" | "use_local" | "keep_offline"> {
  const locale = await desktopLocale.resolve();
  const dialogInput = defaultRuntimeHostRecoveryDialog({ ...input, locale });
  const { response } = await showStartupDiagnosticDialog(
    dialogInput.options,
    locale,
    dialogInput.diagnosticDetails,
  );
  return response === 0 ? "retry" : response === 1 ? "use_local" : "keep_offline";
}
