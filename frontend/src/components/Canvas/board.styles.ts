export interface ColumnMeta {
  title: string;
  color: string;
  dotClass: string;
  textClass: string;
  bgClass: string;
}

export const columnConfig: Record<string, ColumnMeta> = {
  ready: {
    title: 'To Do',
    color: '#a1a1aa',
    dotClass: 'bg-zinc-400',
    textClass: 'text-zinc-300',
    bgClass: 'bg-zinc-400/10',
  },
  scheduled: {
    title: 'Scheduled',
    color: '#c084fc',
    dotClass: 'bg-purple-400',
    textClass: 'text-purple-300',
    bgClass: 'bg-purple-400/10',
  },
  running: {
    title: 'Running',
    color: '#38bdf8',
    dotClass: 'bg-sky-400',
    textClass: 'text-sky-300',
    bgClass: 'bg-sky-400/10',
  },
  completed: {
    title: 'Done',
    color: '#34d399',
    dotClass: 'bg-emerald-400',
    textClass: 'text-emerald-300',
    bgClass: 'bg-emerald-400/10',
  },
  failed: {
    title: 'Failed',
    color: '#f87171',
    dotClass: 'bg-red-400',
    textClass: 'text-red-300',
    bgClass: 'bg-red-400/10',
  },
};

export const COLUMN_ORDER = ['ready', 'scheduled', 'running', 'completed', 'failed'];

export const agentStatusDot: Record<string, string> = {
  idle: 'bg-zinc-500',
  working: 'bg-sky-400 shadow-[0_0_8px_#38bdf8]',
  completed: 'bg-emerald-400',
  failed: 'bg-red-400',
};