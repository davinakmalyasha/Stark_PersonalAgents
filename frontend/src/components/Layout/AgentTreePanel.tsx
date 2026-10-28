import { useCallback, useEffect, useMemo, useState } from 'react';
import { Bot, ChevronDown, ChevronRight, X } from 'lucide-react';
import type { AgentNode } from '../../types';
import { cancelAgent, fetchAgents } from '../../services/api';

interface Props {
  collapsed: boolean;
  onOpen: (agent: AgentNode) => void;
  refreshSignal?: number;
}

const STATUS_COLOR: Record<string, string> = {
  running: 'bg-emerald-400',
  queued: 'bg-amber-400',
  idle: 'bg-zinc-500',
  interrupted: 'bg-orange-400',
  completed: 'bg-sky-400',
  failed: 'bg-red-400',
  cancelled: 'bg-zinc-600',
};

export default function AgentTreePanel({ collapsed, onOpen, refreshSignal = 0 }: Props) {
  const [agents, setAgents] = useState<AgentNode[]>([]);
  const [open, setOpen] = useState(true);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const load = useCallback(() => {
    fetchAgents().then(setAgents).catch(() => setAgents([]));
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(load, 5000);
    return () => window.clearInterval(timer);
  }, [load, refreshSignal]);

  const children = useMemo(() => {
    const map = new Map<string | null, AgentNode[]>();
    for (const agent of agents) {
      const key = agent.parentId ?? null;
      map.set(key, [...(map.get(key) ?? []), agent]);
    }
    return map;
  }, [agents]);

  if (collapsed || agents.length === 0) return null;

  const toggle = (id: string) => {
    setExpanded((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const render = (parent: string | null, depth: number) => {
    const rows = children.get(parent) ?? [];
    return rows.map((agent) => {
      const kids = children.get(agent.id) ?? [];
      const isOpen = expanded.has(agent.id);
      const status = STATUS_COLOR[agent.status] ?? 'bg-zinc-500';
      return (
        <div key={agent.id}>
          <div
            className="group flex items-center gap-1.5 rounded-md py-1 pr-1 text-[11px] text-zinc-400 hover:bg-white/[0.04]"
            style={{ paddingLeft: 10 + depth * 12 }}
          >
            {kids.length > 0 ? (
              <button onClick={() => toggle(agent.id)} className="text-zinc-500 hover:text-zinc-200" title={isOpen ? 'Collapse' : 'Expand'}>
                {isOpen ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
              </button>
            ) : <span className="w-[11px]" />}
            <span className={`h-1.5 w-1.5 flex-shrink-0 rounded-full ${status}`} title={agent.status} />
            <button onClick={() => onOpen(agent)} className="min-w-0 flex-1 truncate text-left hover:text-zinc-100" title={`${agent.path} · ${agent.status}`}>
              <span className="text-zinc-300">{agent.nickname || agent.role || agent.scope}</span>
              <span className="ml-1 text-[9px] uppercase tracking-wide text-zinc-600">{agent.kind}</span>
            </button>
            {(agent.status === 'running' || agent.status === 'queued') && (
              <button
                onClick={() => cancelAgent(agent.id).then(load)}
                className="hidden text-zinc-600 hover:text-red-300 group-hover:block"
                title="Cancel agent"
              >
                <X size={10} />
              </button>
            )}
          </div>
          {isOpen && kids.length > 0 && render(agent.id, depth + 1)}
        </div>
      );
    });
  };

  return (
    <div className="mb-2" style={{ padding: '0 8px' }}>
      <button
        onClick={() => setOpen((value) => !value)}
        className="flex w-full items-center gap-1 px-1.5 py-1 text-[11px] font-bold uppercase tracking-widest text-zinc-500 hover:text-zinc-300"
      >
        <Bot size={12} />
        <span>Agents</span>
        <span className="ml-auto text-[9px] text-zinc-600">{agents.length}</span>
        <ChevronDown size={12} className={`transition-transform ${open ? '' : '-rotate-90'}`} />
      </button>
      {open && <div className="mt-1 flex flex-col gap-0.5">{render(null, 0)}</div>}
    </div>
  );
}
