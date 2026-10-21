// ── Assistant identity ──
// Frontend-only display name. The assistant prompt lives in one place:
// agents/orchestrator_agent.py.

const NAME_KEY = 'stark.assistantName';

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
