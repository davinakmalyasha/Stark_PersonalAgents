import { useMemo } from 'react';
import type { CanvasNode, CanvasEdge } from '../types/canvas.types';
import type { TokenUsage } from '../types';
import { parseLogToSteps } from '../utils/logParser';

interface Input {
  isRunning: boolean;
  liveLog: string | undefined;
  getInitialNodes: () => CanvasNode[];
  getInitialEdges: () => CanvasEdge[];
  tokenUsage?: TokenUsage | null;
}

/** Map node ids to the agent names used in token call records */
function normalizeTokenAgent(nodeId: string): string[] {
  const lower = nodeId.toLowerCase();
  switch (lower) {
    case 'supervisor': return ['supervisor', 'orchestrator', 'director'];
    case 'dev': return ['dev', 'developer', 'coder'];
    case 'bsa': return ['bsa', 'business analyst', 'ba'];
    case 'sdet': return ['sdet', 'tester', 'qa'];
    case 'devsecops': return ['devsecops', 'devops'];
    case 'analytics': return ['analytics', 'data'];
    case 'freeagent': return ['freeagent', 'free agent'];
    case 'designer': return ['designer', 'ui/ux'];
    default: return [lower];
  }
}

export function useCanvasAgentState({ isRunning, liveLog, getInitialNodes, getInitialEdges, tokenUsage }: Input) {
  const steps = useMemo(() => {
    if (!liveLog) return [];
    return parseLogToSteps(liveLog).steps;
  }, [liveLog]);

  const activeAgent = useMemo(() => {
    if (!isRunning) return null;
    if (steps.length === 0) return 'Supervisor';
    const lastStep = steps[steps.length - 1];
    if (lastStep.toolName === 'task' || lastStep.toolName === 'start_async_task') {
      try {
        const parsed = JSON.parse(lastStep.args || '{}');
        if (parsed.name) return parsed.name;
      } catch {}
    }
    return lastStep.agent || 'Supervisor';
  }, [isRunning, steps]);

  /** Per-node token totals derived from API call records */
  const nodeTokens = useMemo(() => {
    const totals: Record<string, number> = {};
    if (!tokenUsage?.calls) return totals;
    for (const call of tokenUsage.calls) {
      const agentLower = call.agent.toLowerCase();
      // Match call to a node id via the name map
      const nodeIds = ['Supervisor', 'Dev', 'BSA', 'SDET', 'DevSecOps', 'FreeAgent'];
      for (const nid of nodeIds) {
        if (normalizeTokenAgent(nid).some(k => agentLower.includes(k))) {
          totals[nid] = (totals[nid] || 0) + (call.input || 0) + (call.output || 0);
          break;
        }
      }
    }
    return totals;
  }, [tokenUsage]);

  const nodes = useMemo(() => {
    const base = getInitialNodes();
    return base.map((node) => {
      let status: 'idle' | 'working' | 'completed' | 'failed' = 'idle';
      if (node.id === 'Supervisor') {
        status = isRunning && activeAgent === 'Supervisor' ? 'working' : isRunning ? 'idle' : 'completed';
      } else {
        if (activeAgent === node.id) {
          status = 'working';
        } else if (steps.some((s) => s.agent === node.id)) {
          status = 'completed';
        }
      }

      const agentSteps = steps.filter((s) => s.agent === node.id || (node.id === 'Supervisor' && s.agent === 'Supervisor'));
      const lastStep = agentSteps[agentSteps.length - 1];
      const lastStepText = lastStep ? (lastStep.title || lastStep.toolName) : undefined;

      return { ...node, status, lastStepText, tokenUsage: nodeTokens[node.id] || 0 };
    });
  }, [getInitialNodes, isRunning, activeAgent, steps, nodeTokens]);

  const edges = useMemo(() => {
    const base = getInitialEdges();
    return base.map((edge) => ({
      ...edge,
      active: isRunning && activeAgent === edge.to,
    }));
  }, [getInitialEdges, isRunning, activeAgent]);

  return { steps, activeAgent, nodes, edges };
}
