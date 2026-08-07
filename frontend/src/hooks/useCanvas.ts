import { useState, useCallback } from 'react';
import type { CanvasNode, CanvasEdge } from '../types/canvas.types';

const CENTER_X = 1400;
const CENTER_Y = 580;
const RADIUS = 340;
export const CANVAS_SIZE = { width: 2800, height: 1200 };
export const CANVAS_LIMITS = { minZoom: 0.65, maxZoom: 1.6 };

const CIRCLE_AGENTS = ['BSA', 'Dev', 'SDET', 'DevSecOps', 'FreeAgent'];

const AGENT_LABELS: Record<string, string> = {
  Dev: 'Developer Subagent',
  BSA: 'Business Analyst Subagent',
  SDET: 'QA / SDET Subagent',
  DevSecOps: 'DevSecOps Subagent',
  FreeAgent: 'FreeAgent',
};

export function useCanvas() {
  const [pan, setPan] = useState({ x: -900, y: -150 });
  const [zoom, setZoom] = useState(1);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);

  const getInitialNodes = useCallback((): CanvasNode[] => {
    const nodes: CanvasNode[] = [
      { id: 'Supervisor', label: 'Supervisor', type: 'supervisor', status: 'idle', x: CENTER_X, y: CENTER_Y }
    ];

    CIRCLE_AGENTS.forEach((name, i) => {
      const angle = (i * 2 * Math.PI) / CIRCLE_AGENTS.length;
      nodes.push({
        id: name,
        label: AGENT_LABELS[name] || name,
        type: 'subagent',
        status: 'idle',
        x: Math.round(CENTER_X + RADIUS * Math.cos(angle)),
        y: Math.round(CENTER_Y + RADIUS * Math.sin(angle)),
      });
    });

    return nodes;
  }, []);

  const getInitialEdges = useCallback((): CanvasEdge[] => {
    // 1. Connect Supervisor to all subagents (Orchestration lines)
    const edges = CIRCLE_AGENTS.map((name) => ({
      id: `edge-Supervisor-${name}`,
      from: 'Supervisor',
      to: name,
      active: false,
    }));

    // 2. Connect subagents in a sequential workflow cycle representing the development lifecycle:
    // BSA -> Dev -> SDET -> DevSecOps -> BSA
    for (let i = 0; i < CIRCLE_AGENTS.length; i++) {
      const fromAgent = CIRCLE_AGENTS[i];
      const toAgent = CIRCLE_AGENTS[(i + 1) % CIRCLE_AGENTS.length];
      edges.push({
        id: `edge-flow-${fromAgent}-${toAgent}`,
        from: fromAgent,
        to: toAgent,
        active: false,
      });
    }

    return edges;
  }, []);

  const handleZoom = useCallback((factor: number) => {
    setZoom((z) => Math.max(CANVAS_LIMITS.minZoom, Math.min(CANVAS_LIMITS.maxZoom, z * factor)));
  }, []);

  const handleReset = useCallback(() => {
    setPan({ x: -900, y: -150 });
    setZoom(1);
  }, []);

  return {
    pan,
    setPan,
    zoom,
    setZoom,
    selectedNodeId,
    setSelectedNodeId,
    getInitialNodes,
    getInitialEdges,
    handleZoom,
    handleReset,
  };
}
