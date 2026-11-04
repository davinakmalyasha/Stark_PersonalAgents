// Offline outbox: text prompts typed without connectivity are queued and
// sent once the browser comes back online. Files are never queued (they live
// only in memory); the UI says so when a file is attached.

const KEY = 'stark.outbox.v1';

export interface OutboxItem {
  id: string;
  chatId: string;
  workspaceId: string;
  workspacePath: string;
  text: string;
  createdAt: string;
}

function read(): OutboxItem[] {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? (JSON.parse(raw) as OutboxItem[]) : [];
  } catch {
    return [];
  }
}

function write(items: OutboxItem[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(items.slice(-50)));
  } catch {
    /* storage full/unavailable */
  }
}

export function queueMessage(item: Omit<OutboxItem, 'id' | 'createdAt'>): OutboxItem {
  const entry: OutboxItem = {
    ...item,
    id: `out-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    createdAt: new Date().toISOString(),
  };
  write([...read(), entry]);
  return entry;
}

export function pendingCount(): number {
  return read().length;
}

/**
 * Send queued prompts. Returns how many were accepted. A failed send stays in
 * the queue for the next attempt; a 4xx that is not connectivity-related is
 * dropped so one bad item cannot block the rest forever.
 */
export async function flushOutbox(apiBase: string): Promise<number> {
  const items = read();
  if (!items.length) return 0;
  const remaining: OutboxItem[] = [];
  let sent = 0;
  for (const item of items) {
    try {
      const res = await fetch(`${apiBase}/run`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          prompt: item.text,
          model: 'Automatic Fallback',
          chat_id: item.chatId,
          workspace_id: item.workspaceId,
          workspace_path: item.workspacePath,
        }),
      });
      if (res.ok) {
        sent += 1;
      } else if (res.status >= 500 || res.status === 429) {
        remaining.push(item); // transient: keep for later
      }
    } catch {
      remaining.push(item); // offline again
    }
  }
  write(remaining);
  return sent;
}
