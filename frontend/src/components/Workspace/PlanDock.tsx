import { useState } from 'react';
import { ChevronDown, ChevronUp, Check } from 'lucide-react';
import type { PlanItem } from '../../types';
import '../../App.css';

interface Props {
  plan?: Record<string, PlanItem[]>;
}

const SCOPE_LABELS: Record<string, string> = {
  developer: 'dev',
  orchestrator: 'orch',
  free_agent: 'free',
  agent: 'agent',
};

function activeScope(plan: Record<string, PlanItem[]>) {
  const entries = Object.entries(plan).filter(([, items]) => Array.isArray(items) && items.length > 0);
  if (entries.length === 0) return null;
  const withActive = entries.find(([, items]) => items.some((it) => it.status === 'in_progress'));
  return withActive ? withActive[0] : entries[0][0];
}

export default function PlanDock({ plan }: Props) {
  const [collapsed, setCollapsed] = useState(false);
  if (!plan || Object.keys(plan).length === 0) return null;

  const scope = activeScope(plan);
  if (!scope) return null;
  const items = plan[scope];
  const completed = items.filter((it) => it.status === 'completed').length;
  const inProgress = items.filter((it) => it.status === 'in_progress').length;
  const total = items.length;

  const visible = collapsed ? items.filter((it) => it.status !== 'completed').slice(0, 3) : items;

  return (
    <div className="plan-dock">
      <button
        type="button"
        className="plan-dock-header"
        onClick={() => setCollapsed((c) => !c)}
        title={collapsed ? 'Expand plan' : 'Collapse plan'}
      >
        <span className="plan-dock-eyebrow">Plan</span>
        <span className="plan-dock-scope">{SCOPE_LABELS[scope] ?? scope}</span>
        <span className="plan-dock-count">
          {completed}/{total} completed{inProgress > 0 ? ` · ${inProgress} active` : ''}
        </span>
        {collapsed ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
      </button>
      <ul className="plan-dock-list">
        {visible.map((it, i) => (
          <li key={i} className={`plan-item plan-item-${it.status}`}>
            <span className="plan-marker">
              {it.status === 'completed' ? (
                <Check size={11} strokeWidth={3} />
              ) : (
                <span className="plan-dot" />
              )}
            </span>
            <span className="plan-step">{it.step}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}