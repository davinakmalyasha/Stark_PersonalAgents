import { useState } from 'react';
import { ChevronRight, AlertTriangle } from 'lucide-react';
import Markdown from '../../components/Workspace/Markdown';
import type { GroupItem, GroupMilestone } from '../../types';
import { displayActor, money, statusChipClass } from './groupProjectUi';

interface Props {
  milestones: GroupMilestone[];
  items: GroupItem[];
}

function ItemDetail({ item }: { item: GroupItem }) {
  return (
    <div className="gp-item-detail">
      {item.detail && <p>{item.detail}</p>}
      {item.error && <div className="gp-item-error"><AlertTriangle size={12} /> {item.error}</div>}
      {item.result && <div className="gp-item-result"><Markdown content={item.result} /></div>}
      {Array.isArray(item.acceptance) && item.acceptance.length > 0 && (
        <div className="gp-item-acceptance">
          <span>Done means</span>
          <pre>{JSON.stringify(item.acceptance, null, 2)}</pre>
        </div>
      )}
      <div className="gp-item-meta">
        <span>{item.attempts || 0}/{item.max_attempts || 3} attempts</span>
        {item.owner_member && <span>owner {displayActor(item.owner_member)}</span>}
        {typeof item.cost_usd === 'number' && item.cost_usd > 0 && <span>{money(item.cost_usd)}</span>}
      </div>
    </div>
  );
}

export default function GroupProjectBoard({ milestones, items }: Props) {
  const [open, setOpen] = useState<string | null>(null);

  if (milestones.length === 0) {
    return <div className="gp-rail-empty">No milestones yet.</div>;
  }

  return (
    <div className="gp-board">
      {milestones.map((milestone) => {
        const mine = items.filter((item) => item.milestone_id === milestone.id);
        return (
          <section key={milestone.id} className="gp-board-milestone">
            <div className="gp-board-milestone-head">
              <ChevronRight size={12} className="text-zinc-600" />
              <span>{milestone.title}</span>
              <span className={statusChipClass(milestone.status)}>{milestone.status}</span>
            </div>
            <div className="gp-board-items">
              {mine.map((item) => (
                <div key={item.id} className="gp-board-item-wrap">
                  <button
                    type="button"
                    className="gp-board-item"
                    onClick={() => setOpen((value) => (value === item.id ? null : item.id))}
                    aria-expanded={open === item.id}
                  >
                    <span className="gp-board-item-id">{item.id}</span>
                    <span className="gp-board-item-title">{item.title}</span>
                    {item.lease_owner && (
                      <span className="gp-board-item-owner">{displayActor(item.lease_owner)}</span>
                    )}
                    <span className={statusChipClass(item.status, item.blocked_reason === 'awaiting_approval')}>
                      {item.blocked_reason === 'awaiting_approval' ? 'needs you' : item.status}
                    </span>
                  </button>
                  {open === item.id && <ItemDetail item={item} />}
                </div>
              ))}
              {mine.length === 0 && <div className="gp-rail-empty">No items yet.</div>}
            </div>
          </section>
        );
      })}
    </div>
  );
}
