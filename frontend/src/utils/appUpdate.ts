// Startup auto-update: check the signed release feed, prompt once per day,
// download + install in the background, relaunch to finish. The updater
// covers the app and the stark-server sidecar. No-ops outside Tauri.
export async function checkForAppUpdate(notify: (message: string) => void): Promise<void> {
  try {
    const last = localStorage.getItem('stark-update-check');
    if (last && Date.now() - Number(last) < 24 * 3600 * 1000) return;
    localStorage.setItem('stark-update-check', String(Date.now()));
  } catch {
    return;
  }
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
    notify('Update download failed. It will retry tomorrow.');
    return;
  }
  notify(`Stark ${update.version} installed — restarting…`);
  await proc.relaunch().catch(() => {});
}
