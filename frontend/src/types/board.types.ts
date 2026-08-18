export type BoardAgentStatus = 'idle' | 'working' | 'completed' | 'failed';

export interface BoardAgent {
  id: string;
  label: string;
  role: 'Orchestrator' | 'Subagent';
  status: BoardAgentStatus;
  task: string;
  steps: number;
  thoughts: number;
  tools: number;
  tokens: number;
  isActive: boolean;
}