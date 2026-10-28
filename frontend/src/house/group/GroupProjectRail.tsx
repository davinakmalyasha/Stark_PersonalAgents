import { useMemo, useState } from 'react';
import { AlertTriangle, Check, ChevronRight, Clock, FolderOpen, Shield, Users, X } from 'lucide-react';
import type { GroupEvent, GroupItem, GroupMember, GroupMilestone, GroupProject } from '../../types';
import GroupProjectBoard from './GroupProjectBoard';
import {
  activeItemFor, avatarTone, displayActor, eventLabel, initials, money,
  statusChipClass, timeAgo,
} from './groupProjectUi';

interface Props {
  project: GroupProject;
  path?: string;
  shellEnabled?: boolean;
  milestones: GroupMilestone[];
  items: GroupItem[];
  events: GroupEvent[];
  members?: GroupMember[];
  waiting: GroupItem[];
  busyItem: string | null;
  onResolve: (itemId: string, grant: boolean) => void;
  onOpenFolder: () => void;
}

type RailTab = 'board' | 'team' | 'details';

function Avatar({ name }: { name: string }) {
  return <span className={`gp-avatar gp-avatar--sm ${avatarTone(name)}`}>{initials(name)}</span>;
}

export default function GroupProjectRail({
  project, path, shellEnabled, milestones, items, events, members, waiting,
  busyItem, onResolve, onOpenFolder,
}: Props) {
  const [tab, setTab] = useState<RailTab>('board');
  const [showActivity, setShowActivity] = useState(false);
  const projectStatus = String(project.status || 'unknown');
  const milestonesDone = milestones.filter((m) => m.status === 'done').length;
  const itemsDone = items.filter((i) => i.status === 'done').length;
  const spent = Number(project.spent_usd || 0);
  const cap = project.budget_usd ? Number(project.budget_usd) : null;
  const progress = items.length ? Math.round((itemsDone / items.length) * 100) : 0;
  const team = useMemo<GroupMember[]>(() => {
    if (members?.length) return members;
    const names = new Set<string>();
    for (const item of items) {
      if (item.owner_member) names.add(item.owner_member);
      if (item.lease_owner) names.add(item.lease_owner);
    }
    return Array.from(names).map((name) => ({ name, display: displayActor(name), kind: 'member' }));
  }, [members, items]);

  return (
    <aside className="gp-rail">
      <div className="gp-rail-scroll">
        <section className="gp-rail-card gp-progress-card">
          <div className="gp-rail-card-head">
            <span>Progress</span>
            <span className={statusChipClass(projectStatus)}>{projectStatus}</span>
          </div>
          <div className="gp-progress-number">{progress}%</div>
          <div className="gp-progress-bar"><div style={{ width: `${progress}%` }} /></div>
          <div className="gp-progress-stats">
            <span><b>{milestonesDone}/{milestones.length}</b> milestones</span>
            <span><b>{itemsDone}/{items.length}</b> items</span>
          </div>
          <div className="gp-progress-spend">
            <span>Spend</span>
            <span><b>{money(spent)}</b>{cap ? <small> / {money(cap)}</small> : null}</span>
          </div>
          {typeof project.deadline === 'number' && project.deadline > 0 && (
            <div className="gp-progress-deadline"><Clock size={11} /> {new Date(project.deadline * 1000).toLocaleString()}</div>
          )}
        </section>

        {waiting.length > 0 && (
          <section className="gp-rail-card gp-waiting-card">
            <div className="gp-rail-section-title"><AlertTriangle size={12} /> Waiting on you</div>
            {waiting.map((item) => (
              <div key={item.id} className="gp-waiting-row">
                <div className="gp-waiting-title">{item.title}</div>
                <div className="gp-waiting-id">{item.id}</div>
                <div className="gp-waiting-actions">
                  <button
                    type="button"
                    className="gp-button gp-button--primary"
                    disabled={busyItem === item.id}
                    onClick={() => onResolve(item.id, true)}
                  >
                    <Check size={12} /> Approve
                  </button>
                  <button
                    type="button"
                    className="gp-button"
                    disabled={busyItem === item.id}
                    onClick={() => onResolve(item.id, false)}
                  >
                    <X size={12} /> Reject
                  </button>
                </div>
              </div>
            ))}
          </section>
        )}

        <div className="gp-rail-tabs">
          <button type="button" className={tab === 'board' ? 'is-active' : ''} onClick={() => setTab('board')}>
            Board
          </button>
          <button type="button" className={tab === 'team' ? 'is-active' : ''} onClick={() => setTab('team')}>
            Team <span>{team.length}</span>
          </button>
          <button type="button" className={tab === 'details' ? 'is-active' : ''} onClick={() => setTab('details')}>
            Details
          </button>
        </div>

        {tab === 'board' && <GroupProjectBoard milestones={milestones} items={items} />}

        {tab === 'team' && (
          <section className="gp-team">
            {team.length === 0 ? (
              <div className="gp-rail-empty">No named members.</div>
            ) : team.map((member) => {
              const active = activeItemFor(items, member.name);
              return (
                <div key={member.name} className="gp-team-row">
                  <Avatar name={member.name} />
                  <div className="min-w-0 flex-1">
                    <div className="gp-team-name">{member.display || displayActor(member.name)}</div>
                    <div className="gp-team-task">
                      {active ? active.title : member.model || (member.kind || 'member')}
                    </div>
                  </div>
                  <span className={`gp-team-dot ${active ? 'is-active' : ''}`} />
                </div>
              );
            })}
          </section>
        )}

        {tab === 'details' && (
          <section className="gp-rail-card gp-details">
            <div className="gp-rail-section-title"><Users size={12} /> Project</div>
            {project.goal && <p className="gp-details-goal">{String(project.goal)}</p>}
            <div className="gp-detail-row"><span>Status</span><b>{projectStatus}</b></div>
            <div className="gp-detail-row"><span>Milestones</span><b>{milestonesDone}/{milestones.length}</b></div>
            <div className="gp-detail-row"><span>Items</span><b>{itemsDone}/{items.length}</b></div>
            <div className="gp-detail-row"><span>Spend</span><b>{money(spent)}{cap ? ` / ${money(cap)}` : ''}</b></div>
            {shellEnabled && (
              <div className="gp-detail-note"><Shield size={12} /> Members can run shell commands.</div>
            )}
            {path && (
              <button type="button" className="gp-detail-folder" onClick={onOpenFolder} title={path}>
                <FolderOpen size={12} /> <span>{path}</span>
              </button>
            )}
            <button
              type="button"
              className="gp-activity-toggle"
              onClick={() => setShowActivity((value) => !value)}
              aria-expanded={showActivity}
            >
              <ChevronRight size={12} className={showActivity ? 'rotate-90' : ''} />
              Activity · {events.length}
            </button>
            {showActivity && (
              <div className="gp-activity-list">
                {events.slice(-30).reverse().map((event) => (
                  <div key={event.seq} className="gp-activity-row">
                    <span>{timeAgo(event.ts)}</span>
                    <span>{eventLabel(event.kind)}</span>
                    <span>{displayActor(event.actor)}</span>
                  </div>
                ))}
                {events.length === 0 && <div className="gp-rail-empty">Nothing logged yet.</div>}
              </div>
            )}
          </section>
        )}
      </div>
    </aside>
  );
}
