export const statusConfig = {
  idle: {
    border: 'border-white/[0.11] hover:border-white/[0.22]',
    bg: 'bg-[#202126]/90',
    dot: 'bg-zinc-500',
    iconBg: 'bg-white/[0.07] text-zinc-300 border border-white/[0.08]',
    glow: 'shadow-[0_18px_42px_rgba(0,0,0,0.22)]',
    badge: 'bg-zinc-900/65 text-zinc-500 border-zinc-800/60',
  },
  working: {
    border: 'border-indigo-500/50 hover:border-indigo-400',
    bg: 'bg-gradient-to-br from-[#25242c]/95 via-[#202027]/95 to-indigo-950/20',
    dot: 'bg-indigo-500 shadow-[0_0_8px_#6366f1]',
    iconBg: 'bg-indigo-500/10 text-indigo-400 border border-indigo-500/25',
    glow: 'shadow-[0_0_20px_rgba(99,102,241,0.12)]',
    badge: 'bg-indigo-500/10 text-indigo-400 border-indigo-500/25 animate-pulse',
  },
  completed: {
    border: 'border-emerald-500/40 hover:border-emerald-400',
    bg: 'bg-gradient-to-br from-[#252a29]/95 to-emerald-950/15',
    dot: 'bg-emerald-500 shadow-[0_0_6px_#10b981]',
    iconBg: 'bg-emerald-500/10 text-emerald-400 border border-emerald-500/20',
    glow: 'shadow-[0_4px_24px_rgba(0,0,0,0.45)]',
    badge: 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20',
  },
  failed: {
    border: 'border-red-500/40 hover:border-red-400',
    bg: 'bg-gradient-to-br from-zinc-950/95 to-red-950/15',
    dot: 'bg-red-500 shadow-[0_0_8px_#ef4444]',
    iconBg: 'bg-red-500/10 text-red-400 border border-red-500/25',
    glow: 'shadow-[0_0_20px_rgba(239,68,68,0.12)]',
    badge: 'bg-red-500/10 text-red-400 border-red-500/25',
  },
};
