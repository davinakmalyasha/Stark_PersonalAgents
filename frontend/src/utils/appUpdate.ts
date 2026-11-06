// Desktop auto-update: check the signed release feed, download, install, and
// restart — no dialogs, no Settings detour. Runs from AppShell so it applies
// on every route (home included, which previously never checked).
// Offline or an unreachable feed stays silent unless manual. No-ops on web.
import { setUpdateState, getUpdateState } from './updateStore';
import { anyBusy, onBusyChange } from './runtimeBusy';

let inFlight = false;
let relaunchWhenIdle = false;
let unsubscribeBusy: (() => void) | null = null;

function relaunchSoon(relaunch: () => Promise<void>) {
  setUpdateState({
    phase: 'ready',
    restartRequired: true,
    message: 'Update installed — restarting Stark…',
  });
  window.setTimeout(() => {
    void relaunch().catch(() => {
      setUpdateState({ message: 'Update installed — restart Stark to finish.' });
    });
  }, 1200);
}

async function applyUpdate(update: NonNullable<Awaited<ReturnType<typeof import('@tauri-apps/plugin-updater').check>>>) {
  setUpdateState({
    phase: 'downloading',
    version: update.version,
    restartRequired: false,
    message: `Downloading Stark ${update.version}…`,
  });
  try {
    await update.downloadAndInstall();
  } catch {
    setUpdateState({
      phase: 'error',
      message: 'Update download failed — it will retry later.',
    });
    return;
  }

  const { relaunch } = await import('@tauri-apps/plugin-process');
  if (!anyBusy()) {
    relaunchSoon(relaunch);
    return;
  }

  // A run is active: apply silently, then restart as soon as everything is
  // idle. The user never has to remember to check again.
  relaunchWhenIdle = true;
  setUpdateState({
    phase: 'ready',
    restartRequired: true,
    message: `Stark ${update.version} ready — will restart when this run finishes.`,
  });
  unsubscribeBusy?.();
  unsubscribeBusy = onBusyChange(() => {
    if (!relaunchWhenIdle || anyBusy()) return;
    relaunchWhenIdle = false;
    unsubscribeBusy?.();
    unsubscribeBusy = null;
    relaunchSoon(relaunch);
  });
}

export async function checkForAppUpdate(options: { manual?: boolean } = {}): Promise<void> {
  const manual = options.manual === true;
  if (inFlight) return;

  let updater: typeof import('@tauri-apps/plugin-updater');
  try {
    updater = await import('@tauri-apps/plugin-updater');
  } catch {
    setUpdateState({
      phase: 'unsupported',
      message: 'Web build — updates are applied when the server deploys.',
    });
    return;
  }

  inFlight = true;
  const before = getUpdateState();
  setUpdateState({ phase: 'checking', message: manual ? 'Checking for updates…' : before.message });

  let update: Awaited<ReturnType<typeof updater.check>>;
  try {
    update = await updater.check();
  } catch {
    setUpdateState({
      phase: 'error',
      message: 'Could not check for updates. Try again later.',
    });
    inFlight = false;
    return;
  }

  if (!update) {
    setUpdateState({
      phase: 'uptodate',
      message: 'Stark is up to date.',
      restartRequired: false,
    });
    inFlight = false;
    return;
  }

  await applyUpdate(update);
  inFlight = false;
}

/** Start the launch check plus a slow periodic check. Returns a cleanup fn. */
export function startAutoUpdate(): () => void {
  void checkForAppUpdate();
  const timer = window.setInterval(() => {
    void checkForAppUpdate();
  }, 6 * 60 * 60 * 1000);
  return () => {
    window.clearInterval(timer);
    unsubscribeBusy?.();
    unsubscribeBusy = null;
  };
}
