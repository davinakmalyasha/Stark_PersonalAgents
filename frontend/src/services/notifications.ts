import { pendingCount } from './outbox';

const LAST_NOTIFIED = 'stark.lastNotified';

/** Ask once, quietly. Returns true when notifications are usable. */
export async function ensureNotificationPermission(): Promise<boolean> {
  if (typeof Notification === 'undefined') return false;
  if (Notification.permission === 'granted') return true;
  if (Notification.permission === 'denied') return false;
  try {
    return (await Notification.requestPermission()) === 'granted';
  } catch {
    return false;
  }
}

/**
 * Notify that a run finished while the page is hidden. Works while the PWA is
 * open in the background; a fully closed tab needs server push, which is not
 * wired yet. Deduplicated by chat + finish window.
 */
export async function notifyRunFinished(chatId: string, title: string, body: string) {
  if (typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
  if (!document.hidden) return;
  const key = `${chatId}:${Math.floor(Date.now() / 5000)}`;
  if (localStorage.getItem(LAST_NOTIFIED) === key) return;
  try {
    localStorage.setItem(LAST_NOTIFIED, key);
  } catch {
    /* ignore */
  }
  const queued = pendingCount();
  const suffix = queued > 0 ? ` · ${queued} queued offline` : '';
  new Notification(title || 'Stark', {
    body: `${body.slice(0, 180)}${body.length > 180 ? '…' : ''}${suffix}`,
    icon: '/icons/pwa-192.png',
    tag: chatId,
  });
}
