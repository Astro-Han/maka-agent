import type { DesktopDiagnosticsDeps } from './main-process-diagnostics.js';
import type { RuntimeHostDesktopManager } from './runtime-host-desktop-manager.js';
import type { DesktopTargetScope } from '../shared/runtime-host-identity.js';

// Cross-module late bindings between the early window path and the Runtime
// Host boot: the window is created while the heavy module graph is still
// evaluating, so pieces the window needs early (diagnostics, quit hooks) read
// the Host-side products through this holder once they exist.
export const bootContext: {
  runtimeHostManager?: RuntimeHostDesktopManager;
  activeRuntimeHostRef?: () => DesktopTargetScope | undefined;
  resolveRuntimeHostDiagnostics?: DesktopDiagnosticsDeps['resolveRuntimeHost'];
  prepareToQuit?: () => Promise<'ready' | 'cancelled'>;
  cleanup?: () => Promise<void>;
} = {};
