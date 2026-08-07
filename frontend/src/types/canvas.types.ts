import type { ChatMessage } from './index';

export interface CanvasNode {
  id: string;
  label: string;
  type: 'supervisor' | 'subagent';
  status: 'idle' | 'working' | 'completed' | 'failed';
  x: number;
  y: number;
  lastStepText?: string;
  tokenUsage?: number;
  thoughts?: string;
}

export interface CanvasEdge {
  id: string;
  from: string;
  to: string;
  active: boolean;
}

export interface SubagentDetails {
  name: string;
  status: 'idle' | 'working' | 'completed' | 'failed';
  task: string;
  history: ChatMessage[];
}
