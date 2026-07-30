import type { TokenCall } from '../../types';

interface CallListProps {
  calls: TokenCall[];
}

export function CallList({ calls }: CallListProps) {
  if (!calls || calls.length === 0) return null;

  return (
    <div className="flex flex-col" style={{ gap: '14px' }}>
      <div className="text-[11px] font-bold uppercase tracking-wider text-zinc-400/80 px-0.5">
        Execution Call Log ({calls.length} calls)
      </div>
      
      <div className="flex flex-col max-h-[360px] overflow-y-auto pr-1.5" style={{ gap: '12px' }}>
        {calls.slice().reverse().map((call, idx) => {
          const hits = call.cache_hits || 0;
          const misses = call.cache_misses || (call.input - hits);
          const totalInput = call.input;
          
          const hitPercent = totalInput > 0 ? (hits / totalInput) * 100 : 0;
          const cost = call.cost;

          return (
            <div
              key={idx}
              className="flex flex-col hover:-translate-y-0.5 transition-all duration-200"
              style={{
                padding: '16px',
                borderRadius: '12px',
                border: '1px solid rgba(255, 255, 255, 0.08)',
                backgroundColor: 'rgba(255, 255, 255, 0.02)',
                gap: '10px'
              }}
            >
              <div className="flex justify-between items-center">
                <div className="flex items-center gap-2.5">
                  <span className="px-2 py-0.5 rounded font-bold uppercase text-[9px] bg-zinc-800 text-zinc-300 border border-zinc-700/30">
                    {call.agent}
                  </span>
                  <span className="text-[10px] text-zinc-500 font-mono tracking-wide">{call.model}</span>
                </div>
                <div className="flex items-center gap-2.5">
                  <span className="font-bold text-emerald-400 text-xs">${cost.toFixed(6)}</span>
                  <span className="text-[9px] text-zinc-500 font-medium">{call.timestamp}</span>
                </div>
              </div>
              
              {call.where && (
                <div 
                  className="text-[10.5px] text-zinc-300 font-sans"
                  style={{
                    padding: '12px',
                    borderRadius: '8px',
                    border: '1px solid rgba(255, 255, 255, 0.05)',
                    backgroundColor: 'rgba(0, 0, 0, 0.4)',
                    marginTop: '4px',
                    marginBottom: '4px',
                    lineHeight: '1.5'
                  }}
                >
                  <span className="text-zinc-500 font-bold uppercase text-[8px] block mb-1 tracking-wider">Action / Task Context</span>
                  {call.where}
                </div>
              )}

              {/* Cache Hit vs Miss Bar */}
              <div className="flex flex-col" style={{ gap: '8px', marginTop: '4px' }}>
                <div className="flex justify-between text-[9.5px] font-medium text-zinc-400">
                  <span>Cache Hits: <strong className="text-emerald-400 font-semibold">{hits.toLocaleString()}</strong> ({hitPercent.toFixed(0)}%)</span>
                  <span>Miss: <strong className="text-zinc-300 font-semibold">{misses.toLocaleString()}</strong> | Out: <strong className="text-zinc-300 font-semibold">{call.output.toLocaleString()}</strong></span>
                </div>
                <div className="w-full h-1.5 rounded-full overflow-hidden flex bg-zinc-800/80">
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
