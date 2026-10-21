/**
 * Child chats are titled `{task} · {agent}` by the delegation layer. Older
 * rows can stack the suffix twice (`1+3 · General · General`). The DB keeps
 * the full title; this is display-only cleanup so the hierarchy stays legible.
 */
const AGENT_SUFFIX = /\s*·\s*(General|Dev|UI|DevOps)\s*$/i;

export function cleanChatTitle(title: string | null | undefined): string {
  let out = (title || '').trim();
  if (!out) return 'Untitled';
  let previous = '';
  while (out !== previous) {
    previous = out;
    out = out.replace(AGENT_SUFFIX, '').trim();
  }
  return out || title || 'Untitled';
}
