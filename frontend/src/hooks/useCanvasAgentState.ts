import { useMemo } from 'react';
import type { TokenUsage } from '../types';
import type { BoardAgent, BoardAgentStatus } from '../types/board.types';
import { delegationTarget, isDelegationToolName, parseLogToSteps } from '../utils/logParser';

interface Input {
  isRunning: boolean;
  liveLog: string | undefined;
  tokenUsage?: TokenUsage | null;
}

const AGENT_ROSTER: { id: string; label: string; role: BoardAgent['role'] }[] = [
  // id stays 'Supervisor' to match legacy trace labels; the UI shows "Stark".
  { id: 'Supervisor', label: 'Stark', role: 'Orchestrator' },
  { id: 'Dev', label: 'Developer Subagent', role: 'Subagent' },
  { id: 'General', label: 'General Subagent', role: 'Subagent' },
  { id: 'Vision', label: 'Vision Agent', role: 'Subagent' },
];

/** Map node ids to the agent names used in token call records */
function normalizeTokenAgent(nodeId: string): string[] {
  const lower = nodeId.toLowerCase();
  switch (lower) {
    case 'supervisor': return ['supervisor', 'orchestrator', 'director'];
    case 'dev': return ['dev', 'developer', 'coder'];
    case 'devsecops': return ['devsecops', 'devops'];
    case 'analytics': return ['analytics', 'data'];
    case 'general': return ['general', 'freeagent', 'free agent'];
    case 'vision': return ['vision', 'visual'];
    case 'designer': return ['designer', 'ui/ux'];
    default: return [lower];
  }
}

/** Pull the task description from the delegation call targeting the given agent. */
function getAgentTask(steps: ReturnType<typeof parseLogToSteps>['steps'], agentId: string): string {
  for (let i = steps.length - 1; i >= 0; i--) {
    const s = steps[i];
    if (!isDelegationToolName(s.toolName)) continue;
    const target = delegationTarget(s.args);
    if (target.name === agentId && target.task) return target.task;
  }
  return '';
}

export function useCanvasAgentState({ isRunning, liveLog, tokenUsage }: Input) {
  const steps = useMemo(() => {
    if (!liveLog) return [];
    return parseLogToSteps(liveLog).steps;
  }, [liveLog]);

  const activeAgent = useMemo(() => {
    if (!isRunning) return null;
    if (steps.length === 0) return 'Supervisor';
    const lastStep = steps[steps.length - 1];
    if (isDelegationToolName(lastStep.toolName)) {
      const target = delegationTarget(lastStep.args);
      if (target.name) return target.name;
    }
    return lastStep.agent || 'Supervisor';
  }, [isRunning, steps]);

  /** Per-agent token totals derived from API call records */
  const nodeTokens = useMemo(() => {
    const totals: Record<string, number> = {};
    if (!tokenUsage?.calls) return totals;
    for (const call of tokenUsage.calls) {
      const agentLower = call.agent.toLowerCase();
      for (const def of AGENT_ROSTER) {
        if (normalizeTokenAgent(def.id).some((key) => agentLower.includes(key))) {
          totals[def.id] = (totals[def.id] || 0) + (call.input || 0) + (call.output || 0);
          break;
        }
      }
    }
    return totals;
  }, [tokenUsage]);

  const agents = useMemo<BoardAgent[]>(() => {
    return AGENT_ROSTER.map((def) => {
      const agentSteps = steps.filter((s) => s.agent === def.id);
      let status: BoardAgentStatus = 'idle';
      if (isRunning && activeAgent === def.id) {
        status = 'working';
      } else if (agentSteps.length > 0) {
        status = 'completed';
      }

      return {
        ...def,
        status,
        task: getAgentTask(steps, def.id),
        steps: agentSteps.length,
        thoughts: agentSteps.filter((s) => s.type === 'thought').length,
        tools: agentSteps.filter((s) => s.type === 'tool' || s.type === 'command').length,
        tokens: nodeTokens[def.id] || 0,
        isActive: isRunning && activeAgent === def.id,
      };
    });
  }, [isRunning, activeAgent, steps, nodeTokens]);

  return { steps, activeAgent, agents };
}