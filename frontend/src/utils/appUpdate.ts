// Startup auto-update: check the signed release feed on every launch,
// prompt once, download + install in the background, relaunch to finish.
// The updater covers the app and the stark-server sidecar. No-ops outside
// Tauri. Offline or an unreachable feed stays silent.
export async function checkForAppUpdate(notify: (message: string) => void): Promise<void> {
  let updater: typeof import('@tauri-apps/plugin-updater');
  let dialog: typeof import('@tauri-apps/plugin-dialog');
  let proc: typeof import('@tauri-apps/plugin-process');
  try {
    [updater, dialog, proc] = await Promise.all([
      import('@tauri-apps/plugin-updater'),
      import('@tauri-apps/plugin-dialog'),
      import('@tauri-apps/plugin-process'),
    ]);
  } catch {
    return; // not running inside Tauri
  }
  let update: Awaited<ReturnType<typeof updater.check>> = null;
  try {
    update = await updater.check();
  } catch {
    return; // offline or feed unreachable: stay silent
  }
  if (!update) return;
  const ok = await dialog
    .confirm(`Stark ${update.version} is available. Install now and restart?`, 'Update available')
    .catch(() => false);
  if (!ok) return;
  notify(`Downloading Stark ${update.version}…`);
  try {
    await update.downloadAndInstall();
  } catch {
    notify('Update download failed. It will retry next launch.');
    return;
  }
  notify(`Stark ${update.version} installed — restarting…`);
  await proc.relaunch().catch(() => {});
}
