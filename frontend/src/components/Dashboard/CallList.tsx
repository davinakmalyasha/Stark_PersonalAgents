import type { TokenCall } from '../../types';

interface CallListProps {
  calls: TokenCall[];
}

export function CallList({ calls }: CallListProps) {
  if (!calls || calls.length === 0) return null;

  return (
    <div className="flex flex-col" style={{ gap: '10px' }}>
      <div className="text-[11px] font-bold uppercase tracking-wider text-zinc-400/80 px-0.5">
        Execution Call Log ({calls.length} calls)
      </div>
      
      <div className="flex flex-col max-h-[620px] overflow-y-auto pr-1.5" style={{ gap: '8px' }}>
        {calls.slice().reverse().map((call, idx) => {
          const hits = call.cache_hits || 0;
          const misses = call.cache_misses || (call.input - hits);
          const totalInput = call.input;
          
          const hitPercent = totalInput > 0 ? (hits / totalInput) * 100 : 0;
          const cost = call.cost;

          return (
            <div
              key={idx}
              className="group relative flex flex-col overflow-hidden transition-all duration-200 hover:border-zinc-600/70 hover:bg-white/[0.035]"
              style={{
                padding: '14px',
                borderRadius: '14px',
                border: '1px solid rgba(255, 255, 255, 0.09)',
                background: 'linear-gradient(145deg, rgba(255,255,255,.045), rgba(255,255,255,.018) 55%, rgba(0,0,0,.12))',
                boxShadow: 'inset 0 1px 0 rgba(255,255,255,.04)',
                gap: '12px'
              }}
            >
              <div className="absolute left-0 top-0 bottom-0 w-0.5 bg-zinc-500/70 opacity-70 transition-opacity group-hover:opacity-100" />

              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 items-center gap-2.5">
                  <span className="shrink-0 rounded-md bg-zinc-800/90 px-2 py-1 font-bold uppercase text-[9px] tracking-wide text-zinc-200 ring-1 ring-inset ring-white/10">
                    {call.agent}
                  </span>
                  <span className="truncate font-mono text-[10px] tracking-wide text-zinc-500">{call.model}</span>
                </div>
                <div className="flex shrink-0 items-baseline gap-2.5">
                  <span className="font-bold text-xs text-zinc-100 tabular-nums">${cost.toFixed(6)}</span>
                  <span className="font-medium text-[9px] text-zinc-500 tabular-nums">{call.timestamp}</span>
                </div>
              </div>
              
              {call.where && (
                <div 
                  className="text-[10.5px] text-zinc-300 font-sans"
                  style={{
                    padding: '11px 12px',
                    borderRadius: '10px',
                    border: '1px solid rgba(255, 255, 255, 0.07)',
                    background: 'rgba(0, 0, 0, 0.26)',
                    lineHeight: '1.5'
                  }}
                >
                  <span className="mb-1 block text-[8px] font-bold uppercase tracking-[0.14em] text-zinc-500">Action / Task Context</span>
                  <span className="text-zinc-300">{call.where}</span>
                </div>
              )}

              {/* Cache Hit vs Miss Bar */}
              <div className="flex flex-col" style={{ gap: '8px' }}>
                <div className="flex items-center justify-between gap-3 text-[9.5px] font-medium text-zinc-400">
                  <span>Cache Hits: <strong className="font-semibold text-zinc-200">{hits.toLocaleString()}</strong> <span className="text-zinc-500">({hitPercent.toFixed(0)}%)</span></span>
                  <span className="text-right">Miss: <strong className="font-semibold text-zinc-300">{misses.toLocaleString()}</strong> <span className="text-zinc-600">|</span> Out: <strong className="font-semibold text-zinc-300">{call.output.toLocaleString()}</strong></span>
                </div>
                <div className="flex h-2 w-full overflow-hidden rounded-full bg-zinc-800/90 ring-1 ring-inset ring-white/5">
                  {hits > 0 && (
                    <div className="h-full bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.3)] transition-all duration-500" style={{ width: `${hitPercent}%` }} title={`Cached: ${hits}`} />
                  )}
                  {misses > 0 && (
                    <div className="h-full bg-zinc-600 transition-all duration-500" style={{ width: `${100 - hitPercent}%` }} title={`Uncached: ${misses}`} />
                  )}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
