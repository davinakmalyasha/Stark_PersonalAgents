// ── Assistant identity ──
// Frontend-only config with best-effort memory seeding via the existing
// general-profile API (no backend changes).

const NAME_KEY = 'stark.assistantName';
const MEMORY_KEY = 'stark.memorySeeded.v1';

export const ASSISTANT_NAME = 'Stark';

export function getAssistantName(): string {
  try {
    return localStorage.getItem(NAME_KEY) || ASSISTANT_NAME;
  } catch {
    return ASSISTANT_NAME;
  }
}

export function setAssistantName(name: string): void {
  try {
    localStorage.setItem(NAME_KEY, name);
  } catch { /* storage unavailable */ }
}

/**
 * Teach the general agent its identity and address style by writing into
 * the general profile (read by FreeAgent on every General-mode run):
 *  - memory: the assistant's own name (Reminder)
 *  - instructions: always address the user as Sir (Preferences)
 * Runs once per browser.
 */
export async function seedAssistantMemory(): Promise<void> {
  try {
    if (localStorage.getItem(MEMORY_KEY)) return;
    const name = getAssistantName();
    const res = await fetch('/api/settings/general-profile');
    const profile = res.ok ? await res.json() : {};
    const memory: string[] = Array.isArray(profile.memory) ? profile.memory : [];
    const reminder = `Your name is ${name}. Always refer to yourself as ${name} when speaking.`;
    if (!memory.some((m) => m.includes(name))) {
      memory.push(reminder);
    }
    const instruction = 'Always address the user as "Sir" (like JARVIS addressing Tony Stark) in every reply.';
    const instructions = typeof profile.instructions === 'string'
      ? (profile.instructions.includes('Sir') ? profile.instructions : `${profile.instructions}\n${instruction}`.trim())
      : instruction;
    await fetch('/api/settings/general-profile', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ...profile, instructions, memory }),
    });
    localStorage.setItem(MEMORY_KEY, '1');
  } catch { /* memory seeding is best-effort */ }
}
