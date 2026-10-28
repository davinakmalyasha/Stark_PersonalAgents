import { Bot, X } from 'lucide-react';
import SubagentSessionView from './SubagentSessionView';

export interface OpenSubagentTab {
  childId: string;
  label: string;
  workspaceId?: string;
}

interface Props {
  tabs: OpenSubagentTab[];
  activeChildId: string;
  parentTitle: string;
  fallbackWorkspaceId: string;
  onActivate: (childId: string) => void;
  onClose: (childId: string) => void;
  onOpenSubagent: (childId: string, label: string, workspaceId?: string) => void;
}

export default function SubagentTabsPane({
  tabs,
  activeChildId,
  parentTitle,
  fallbackWorkspaceId,
  onActivate,
  onClose,
  onOpenSubagent,
}: Props) {
  const active = tabs.find((tab) => tab.childId === activeChildId);
  if (!active) return null;

  return (
    <aside className="subagent-session-pane" aria-label="Subagent sessions">
      <header className="subagent-pane-tabs" role="tablist" aria-label="Open subagent chats">
        {tabs.map((tab) => (
          <div key={tab.childId} role="presentation" className={`subagent-pane-tab ${tab.childId === activeChildId ? 'is-active' : ''}`}>
            <button
              type="button"
              role="tab"
              aria-selected={tab.childId === activeChildId}
              onClick={() => onActivate(tab.childId)}
              title={tab.label}
              className="subagent-pane-tab-select"
            >
              <Bot size={13} aria-hidden="true" />
              <span className="subagent-pane-tab-copy"><strong>{tab.label}</strong><small>{parentTitle}</small></span>
            </button>
            <button type="button" onClick={() => onClose(tab.childId)} aria-label={`Close ${tab.label}`} title="Close tab" className="subagent-pane-tab-close">
              <X size={12} />
            </button>
          </div>
        ))}
      </header>
      <div className="subagent-pane-body">
        <SubagentSessionView
          key={`${active.workspaceId || fallbackWorkspaceId}:${active.childId}`}
          workspaceId={active.workspaceId || fallbackWorkspaceId}
          childId={active.childId}
          label={active.label}
          parentTitle={parentTitle}
          hideHeader
          onBack={() => onClose(active.childId)}
          onOpenSubagentSession={onOpenSubagent}
        />
      </div>
    </aside>
  );
}
