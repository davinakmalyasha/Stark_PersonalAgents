// Startup auto-update: check the signed release feed on every launch,
// download + install in the background, relaunch when idle. No confirm
// dialogs — the update applies itself. The updater covers the app and the
// stark-server sidecar. No-ops outside Tauri. Offline or an unreachable feed
// stays silent unless manual.
export async function checkForAppUpdate(
  notify: (message: string) => void,
  options: { manual?: boolean; isBusy?: () => boolean } = {},
): Promise<void> {
  const manual = options.manual === true;
  let updater: typeof import('@tauri-apps/plugin-updater');
  let proc: typeof import('@tauri-apps/plugin-process');
  try {
    [updater, proc] = await Promise.all([
      import('@tauri-apps/plugin-updater'),
      import('@tauri-apps/plugin-process'),
    ]);
  } catch {
    if (manual) notify('Updates are only available in the desktop app.');
    return; // not running inside Tauri
  }
  let update: Awaited<ReturnType<typeof updater.check>> = null;
  try {
    update = await updater.check();
  } catch {
    if (manual) notify('Could not check for updates. Try again later.');
    return; // offline or feed unreachable
  }
  if (!update) {
    if (manual) notify('Stark is up to date.');
    return;
  }
  notify(`Downloading Stark ${update.version}…`);
  try {
    await update.downloadAndInstall();
  } catch {
    notify('Update download failed. It will retry next launch.');
    return;
  }
  if (options.isBusy?.()) {
    notify(`Stark ${update.version} ready — restart to finish.`);
    return;
  }
  notify(`Stark ${update.version} installed — restarting…`);
  await proc.relaunch().catch(() => {});
}
