import type { GroupEvent, GroupItem, GroupMember } from '../../types';

export const GROUP_EVENT_LABELS: Record<string, string> = {
  'project.created': 'Project created',
  'project.paused': 'Paused',
  'project.resumed': 'Resumed',
  'project.closed': 'Closed',
  'project.stalled': 'Stalled — no possible progress',
  'milestone.created': 'Milestone added',
  'milestone.ready': 'Milestone started',
  'milestone.done': 'Milestone done',
  'milestone.failed': 'Milestone failed',
  'item.created': 'Item added',
  'item.ready': 'Item ready',
  'item.claimed': 'Item claimed',
  'item.released': 'Item released',
  'item.progress': 'Progress',
  'item.done': 'Item done',
  'item.failed': 'Item failed',
  'item.blocked': 'Item blocked',
  'item.invalidated': 'Item invalidated — plan was wrong',
  'item.cancelled': 'Item cancelled',
  'plan.revised': 'Plan revised',
  'idea.proposed': 'Idea proposed',
  'op.submitted': 'Change queued',
  'op.applied': 'Change applied',
  'op.rejected': 'Change rejected',
  'lease.expired': 'Worker stopped responding',
  'budget.warn': 'Budget warning',
  'budget.stop': 'Budget ceiling reached',
  'deadline.reached': 'Deadline reached',
  'approval.requested': 'Approval requested',
  'approval.granted': 'Approved',
  'approval.rejected': 'Rejected',
  'member.added': 'Member added',
  'member.removed': 'Member removed',
  'member.halted': 'Member halted',
  'capability.changed': 'Capability changed',
  'message.sent': 'Message',
  'message.read': 'Read',
};

export function eventLabel(kind: string): string {
  return GROUP_EVENT_LABELS[kind] || kind;
}

export function statusChipClass(status: string, awaiting = false): string {
  if (awaiting) return 'gp-chip is-waiting';
  switch (status) {
    case 'done': return 'gp-chip is-done';
    case 'doing': return 'gp-chip is-doing';
    case 'failed': return 'gp-chip is-failed';
    case 'blocked': return 'gp-chip is-blocked';
    case 'invalidated': return 'gp-chip is-invalidated';
    case 'paused': return 'gp-chip is-paused';
    default: return 'gp-chip';
  }
}

export function timeAgo(seconds: number): string {
  if (!seconds) return '';
  const ms = Date.now() - seconds * 1000;
  const mins = Math.floor(ms / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function shortTime(seconds: number): string {
  if (!seconds) return '';
  return new Date(seconds * 1000).toLocaleTimeString([], {
    hour: '2-digit', minute: '2-digit',
  });
}

export function money(value: number): string {
  return value >= 1 ? `$${value.toFixed(2)}` : `$${value.toFixed(4)}`;
}

export function displayActor(value: string): string {
  const raw = (value || '').trim();
  if (!raw) return 'Stark';
  if (raw.toLowerCase().startsWith('member:')) {
    const member = raw.slice(7).trim();
    return member ? member.charAt(0).toUpperCase() + member.slice(1) : 'Member';
  }
  if (raw.toLowerCase() === 'supervisor') return 'Stark';
  if (raw.toLowerCase() === 'owner' || raw.toLowerCase() === 'human') return 'You';
  return raw === 'orchestrator' ? 'Orchestrator' : raw;
}

export function isSupervisor(value: string): boolean {
  const raw = (value || '').trim();
  if (raw.toLowerCase().startsWith('member:')) return false;
  const name = raw.toLowerCase();
  return name === 'supervisor' || name === 'stark';
}

export function initials(value: string): string {
  const name = displayActor(value);
  const parts = name.split(/[\s_-]+/).filter(Boolean);
  if (parts.length === 0) return 'S';
  return (parts[0][0] + (parts[1]?.[0] || '')).toUpperCase();
}

export function avatarTone(value: string): string {
  const name = displayActor(value).toLowerCase();
  if (name === 'stark') return 'gp-avatar--stark';
  if (name.includes('dev')) return 'gp-avatar--dev';
  if (name.includes('general')) return 'gp-avatar--general';
  if (name.includes('house')) return 'gp-avatar--house';
  const tones = ['gp-avatar--a', 'gp-avatar--b', 'gp-avatar--c'];
  let hash = 0;
  for (let i = 0; i < name.length; i += 1) hash = (hash + name.charCodeAt(i)) % tones.length;
  return tones[hash];
}

export function memberFor(members: GroupMember[] | undefined, name: string): GroupMember | undefined {
  if (!members?.length) return undefined;
  const target = displayActor(name).toLowerCase();
  return members.find((member) =>
    member.name.toLowerCase() === target
    || (member.display || '').toLowerCase() === target,
  );
}

export function activeItemFor(items: GroupItem[], member: string): GroupItem | undefined {
  const name = displayActor(member).toLowerCase();
  return items.find((item) =>
    (item.status === 'doing' || item.status === 'ready')
    && (displayActor(String(item.lease_owner || item.owner_member || '')).toLowerCase() === name),
  );
}

export function eventResult(event: GroupEvent): string {
  const data = event.data || {};
  return String(data.result || data.reason || data.detail || '');
}

export function eventError(event: GroupEvent): string {
  const data = event.data || {};
  return String(data.error || '');
}

export interface ChatEntry {
  id: string;
  kind: 'message' | 'activity' | 'approval';
  ts: number;
  from: string;
  to: string;
  body: string;
  messageKind: string;
  itemId?: string | null;
  error?: string;
  derived?: boolean;
}

/** Build a readable transcript for projects created before message events. */
export function deriveChatEntries(events: GroupEvent[], items: GroupItem[]): ChatEntry[] {
  const byId = new Map(items.map((item) => [item.id, item]));
  const out: ChatEntry[] = [];
  for (const event of events) {
    const item = event.item_id ? byId.get(event.item_id) : undefined;
    if (event.kind === 'item.claimed') {
      const owner = displayActor(String(event.data?.lease_owner || item?.owner_member || 'member'));
      out.push({
        id: `derived-${event.seq}`,
        kind: 'message',
        ts: event.ts,
        from: 'supervisor',
        to: owner,
        body: `Assigned ${event.item_id || 'work item'}${item?.title ? `: ${item.title}` : ''}.`,
        messageKind: 'assignment',
        itemId: event.item_id,
        derived: true,
      });
    } else if (event.kind === 'item.progress') {
      const body = eventResult(event);
      if (body) {
        out.push({
          id: `derived-${event.seq}`,
          kind: 'message',
          ts: event.ts,
          from: event.actor,
          to: 'supervisor',
          body,
          messageKind: eventError(event) ? 'report' : 'report',
          itemId: event.item_id,
          error: eventError(event) || undefined,
          derived: true,
        });
      }
    } else if (event.kind === 'approval.requested') {
      const data = event.data || {};
      const body = String(data.detail || (Array.isArray(data.pending) ? data.pending.join('\n') : '') || 'Waiting for your approval.');
      out.push({
        id: `derived-${event.seq}`,
        kind: 'approval',
        ts: event.ts,
        from: event.actor,
        to: 'owner',
        body,
        messageKind: 'approval',
        itemId: event.item_id,
      });
    } else if (event.kind === 'item.done' || event.kind === 'item.failed' || event.kind === 'milestone.done') {
      out.push({
        id: `derived-${event.seq}`,
        kind: 'activity',
        ts: event.ts,
        from: 'supervisor',
        to: 'all',
        body: event.kind === 'item.done'
          ? `${event.item_id} passed acceptance.`
          : event.kind === 'item.failed'
            ? `${event.item_id} failed.`
            : `${event.milestone_id} complete.`,
        messageKind: 'verdict',
        itemId: event.item_id,
        derived: true,
      });
    }
  }
  return out.sort((a, b) => a.ts - b.ts);
}
