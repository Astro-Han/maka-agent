/**
 * Electron binding for the drag-to-grant overlay.
 *
 * The lifecycle lives in `permission-overlay-controller.ts` (pure and
 * tested); this module is the thin layer that gives it a real window, a
 * real cursor, and the real TCC reads — plus the IPC surface.
 *
 * Four window options are load-bearing together, and dropping any one
 * breaks the gesture rather than merely looking worse:
 *
 *   focusable: false        the card never takes key focus
 *   type: 'panel'           NSPanel — does not claim the frontmost app slot
 *   alwaysOnTop 'screen-saver'   floats above System Settings
 *   showInactive()          shown without activating our app
 *
 * If the card steals focus, System Settings stops being the key window and
 * drops its drop-target highlight mid-drag — the user is left dragging
 * into a window that no longer looks like it will accept anything.
 */

import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { UiLocale } from '@maka/core';
import { openSystemPermissionPane } from '../permissions-actions.js';
import { resolveAppBundle } from './app-bundle.js';
import { getPermissionOverlayCopy } from './permission-overlay-copy.js';
import {
  createPermissionOverlayController,
  isDragGrantPermission,
  type DragGrantPermissionId,
  type PermissionOverlayController,
  type PermissionOverlayWindowLike,
} from './permission-overlay-controller.js';

const requireElectron = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

/**
 * Card geometry, in DIP: a wide, short bar rather than a dialog.
 *
 * 530x109 matches the reference implementation, and the proportion is the
 * point — the card has to sit over the width of the System Settings
 * content pane and read as belonging to the list it is pointing at. A
 * squarer card reads as a floating dialog that happens to be nearby.
 */
const CARD = { width: 530, height: 109 };

type Electron = typeof import('electron');

function overlayAssetDir(): string {
  // dist/main/permission-overlay -> dist/overlay
  return join(here, '..', '..', 'overlay');
}

export interface PermissionOverlayMainDeps {
  /**
   * Resolved UI locale, so the card speaks the same language as the app.
   * Async because it comes from the settings store; the controller needs
   * it synchronously when the page loads, so `start()` refreshes a cached
   * value first (see the wrapper at the bottom of this factory).
   */
  resolveLocale(): Promise<UiLocale>;
}

export function createPermissionOverlayMain(
  deps: PermissionOverlayMainDeps,
): PermissionOverlayController {
  let locale: UiLocale = 'en';
  const electron = requireElectron('electron') as Electron;
  const { BrowserWindow, app, nativeImage, screen, systemPreferences } = electron;

  function isGranted(id: DragGrantPermissionId): boolean {
    if (process.platform !== 'darwin') return false;
    // The non-prompting read: passing `true` would pop the system dialog
    // on every poll tick.
    if (id === 'accessibility') return systemPreferences.isTrustedAccessibilityClient(false);
    return systemPreferences.getMediaAccessStatus('screen') === 'granted';
  }

  function appIconDataUrl(bundlePath: string | null): string | null {
    if (!bundlePath) return null;
    for (const name of ['icon.icns', 'electron.icns']) {
      const candidate = join(bundlePath, 'Contents', 'Resources', name);
      if (!existsSync(candidate)) continue;
      const image = nativeImage.createFromPath(candidate);
      if (image.isEmpty()) continue;
      return image.resize({ width: 64, height: 64 }).toDataURL();
    }
    return null;
  }

  const controller = createPermissionOverlayController({
    platform: process.platform,
    cardSize: CARD,
    getAnchor: () => {
      const point = screen.getCursorScreenPoint();
      const display = screen.getDisplayNearestPoint(point);
      return { x: point.x, y: point.y, workArea: display.workArea };
    },
    openSystemSettings: async (id) => {
      const result = await openSystemPermissionPane(id);
      return result.ok ? { ok: true } : { ok: false, message: result.message ?? result.reason };
    },
    isGranted,
    setInterval: (fn, ms) => setInterval(fn, ms),
    clearInterval: (handle) => clearInterval(handle),
    setTimeout: (fn, ms) => setTimeout(fn, ms),
    clearTimeout: (handle) => clearTimeout(handle),
    buildCardPayload: (id) => {
      const bundle = resolveAppBundle({
        executablePath: app.getPath('exe'),
        platform: process.platform,
        exists: existsSync,
      });
      const bundlePath = bundle.ok ? bundle.bundlePath : null;
      return {
        permission: id,
        appName: app.getName(),
        iconDataUrl: appIconDataUrl(bundlePath),
        draggable: bundlePath !== null,
        copy: serializeCopy(locale, id, app.getName()),
      };
    },
    log: (message) => console.warn(message),
    createWindow: (bounds) => {
      const win = new BrowserWindow({
        ...bounds,
        show: false,
        frame: false,
        transparent: true,
        backgroundColor: '#00000000',
        hasShadow: true,
        roundedCorners: true,
        resizable: false,
        movable: false,
        minimizable: false,
        maximizable: false,
        fullscreenable: false,
        skipTaskbar: true,
        // See the header: these two plus showInactive() are what keep
        // System Settings the key window during the drag.
        focusable: false,
        type: process.platform === 'darwin' ? 'panel' : undefined,
        webPreferences: {
          preload: join(overlayAssetDir(), 'permission-overlay-preload.cjs'),
          nodeIntegration: false,
          contextIsolation: true,
          sandbox: false,
        },
      });
      win.setAlwaysOnTop(true, 'screen-saver');
      // Survives Space switches and Settings going fullscreen; without it
      // the card is hidden with our other windows when Settings activates.
      win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true });
      void win.loadFile(join(overlayAssetDir(), 'permission-overlay.html'));

      const like: PermissionOverlayWindowLike = {
        setBounds: (next) => { if (!win.isDestroyed()) win.setBounds(next); },
        showInactive: () => { if (!win.isDestroyed()) win.showInactive(); },
        isDestroyed: () => win.isDestroyed(),
        destroy: () => { if (!win.isDestroyed()) win.destroy(); },
        send: (channel, payload) => { if (!win.isDestroyed()) win.webContents.send(channel, payload); },
        onReady: (cb) => { win.webContents.once('did-finish-load', cb); },
        onGone: (cb) => {
          win.once('closed', cb);
          win.webContents.once('render-process-gone', cb);
        },
      };
      return like;
    },
  });

  // Refresh the locale before each run so a language change between the
  // app starting and the card opening is reflected. Failing to read it is
  // not a reason to block the flow — the last known value still renders.
  return {
    ...controller,
    async start(id: unknown) {
      try {
        locale = await deps.resolveLocale();
      } catch (error) {
        console.warn('[permission-overlay] locale lookup failed, keeping', locale, error);
      }
      return controller.start(id);
    },
  };
}

function serializeCopy(locale: UiLocale, id: DragGrantPermissionId, appName: string) {
  const copy = getPermissionOverlayCopy(locale, id);
  return {
    headline: copy.headline(appName),
    fallback: copy.fallback,
    granted: copy.granted,
    dismiss: copy.dismiss,
    dragHint: copy.dragHint,
    restartHint: copy.restartHint ?? null,
    noBundle: copy.noBundle,
  };
}

export interface PermissionOverlayIpcDeps {
  controller: PermissionOverlayController;
}

/**
 * The drag itself.
 *
 * `webContents.startDrag` is the only way to hand a file to *another*
 * process: it writes a `kUTTypeFileURL` onto NSPasteboard, which is what
 * makes the drop legible to System Settings. An HTML5 `dragstart` in the
 * renderer stays inside our own process and System Settings never sees it.
 */
export function registerPermissionOverlayIpc(deps: PermissionOverlayIpcDeps): void {
  const electron = requireElectron('electron') as Electron;
  const { ipcMain, app, nativeImage, shell } = electron;

  ipcMain.handle('permissions:startDragOnboarding', async (_event, id: unknown) => {
    return deps.controller.start(id);
  });

  ipcMain.handle('permissions:dismissDragOnboarding', async () => {
    deps.controller.dismiss();
    return { ok: true };
  });

  ipcMain.on('permission-overlay:start-drag', (event, payload: unknown) => {
    if (process.platform !== 'darwin') return;
    const bundle = resolveAppBundle({
      executablePath: app.getPath('exe'),
      platform: process.platform,
      exists: existsSync,
    });
    if (!bundle.ok) {
      // No bundle to drag. The card already renders the manual route in
      // this case; surface it rather than starting a drag that macOS will
      // refuse, and give the user something to act on.
      console.warn(`[permission-overlay] no .app bundle to drag (exe: ${bundle.executablePath})`);
      return;
    }

    const iconDataUrl =
      payload && typeof payload === 'object' && 'iconDataUrl' in payload
        ? (payload as { iconDataUrl?: unknown }).iconDataUrl
        : null;
    let icon = nativeImage.createEmpty();
    if (typeof iconDataUrl === 'string' && iconDataUrl.startsWith('data:image/')) {
      const fromRenderer = nativeImage.createFromDataURL(iconDataUrl);
      if (!fromRenderer.isEmpty()) icon = fromRenderer;
    }
    if (icon.isEmpty()) {
      const fallback = nativeImage.createFromPath(
        join(bundle.bundlePath, 'Contents', 'Resources', 'icon.icns'),
      );
      if (!fallback.isEmpty()) icon = fallback.resize({ width: 64, height: 64 });
    }

    event.sender.startDrag({ file: bundle.bundlePath, icon });
  });

  ipcMain.on('permission-overlay:dismiss', () => {
    deps.controller.dismiss();
  });

  ipcMain.on('permission-overlay:reveal-bundle', () => {
    const bundle = resolveAppBundle({
      executablePath: app.getPath('exe'),
      platform: process.platform,
      exists: existsSync,
    });
    // Dev-mode / unpacked fallback: if we cannot hand the bundle over by
    // drag, at least put the user in front of it in Finder instead of
    // leaving the gesture silently dead.
    shell.showItemInFolder(bundle.ok ? bundle.bundlePath : bundle.executablePath);
  });
}

export { isDragGrantPermission };
