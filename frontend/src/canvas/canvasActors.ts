import { Code2, MessageSquare, Sparkles, type LucideIcon } from 'lucide-react';
import type { ChatSummary, WorkspaceInfo } from '../types';
import { getWorkspaceKind } from '../types';

export type ChatActorKind = 'orchestrator' | 'developer' | 'general';

export interface ChatActor {
  kind: ChatActorKind;
  label: string;
  letter: 'O' | 'D' | 'G';
  icon: LucideIcon;
}

const ORCHESTRATOR: ChatActor = {
  kind: 'orchestrator',
  label: 'Orchestrator',
  letter: 'O',
  icon: MessageSquare,
};

const DEVELOPER: ChatActor = {
  kind: 'developer',
  label: 'Developer',
  letter: 'D',
  icon: Code2,
};

const GENERAL: ChatActor = {
  kind: 'general',
  label: 'General',
  letter: 'G',
  icon: Sparkles,
};

/**
 * One chat = one actor. Delegated child titles end with the agent role
 * (`… · General`, `… · Dev`, `… · UI`), which is the most specific signal we
 * have; otherwise the workspace kind decides: main → Orchestrator, code
 * project → Developer, house/personal/everything else → General.
 */
export function resolveChatActor(
  workspace?: WorkspaceInfo,
  chat?: ChatSummary,
): ChatActor {
  const title = chat?.title || '';
  if (/\s·\s*general\s*$/i.test(title)) return GENERAL;
  if (/\s·\s*(dev|ui|devops)\s*$/i.test(title)) return DEVELOPER;

  if (!workspace) return GENERAL;
  const kind = getWorkspaceKind(workspace);
  if (kind === 'general') return ORCHESTRATOR;
  if (kind === 'project') return DEVELOPER;
  return GENERAL;
}
