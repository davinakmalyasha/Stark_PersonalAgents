import type { TokenCall } from '../../types';

interface CallListProps {
  calls: TokenCall[];
}

export function CallList({ calls }: CallListProps) {
  if (!calls || calls.length === 0) return null;

  const ordered = calls.slice().reverse();

  return (
    <section
      className="flex min-w-0 flex-col overflow-hidden"
      style={{
        borderRadius: '14px',
        border: '1px solid rgba(255,255,255,0.08)',
        backgroundColor: 'rgba(255,255,255,0.02)',
      }}
    >
      <div
        className="flex items-center justify-between gap-3"
        style={{
          padding: '14px 18px 12px',
          borderBottom: '1px solid rgba(255,255,255,0.07)',
        }}
      >
        <span className="text-[11px] font-medium uppercase tracking-[0.12em] text-zinc-500">
          Requests
        </span>
        <span className="text-[11px] text-zinc-500 tabular-nums">
          {calls.length} {calls.length === 1 ? 'call' : 'calls'}
        </span>
      </div>

      <div
        className="flex flex-col overflow-y-auto"
        style={{ maxHeight: '620px' }}
      >
        {ordered.map((call, idx) => {
          const hits = call.cache_hits || 0;
          const misses = call.cache_misses ?? Math.max((call.input || 0) - hits, 0);
          const totalInput = call.input || 0;
          const hitPercent = totalInput > 0 ? (hits / totalInput) * 100 : 0;

          return (
            <div
              key={idx}
              className="flex flex-col"
              style={{
                padding: '14px 18px',
                gap: '10px',
                borderBottom:
                  idx === ordered.length - 1
                    ? 'none'
                    : '1px solid rgba(255,255,255,0.06)',
              }}
            >
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    className="shrink-0 rounded-md px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-zinc-300"
                    style={{
                      border: '1px solid rgba(255,255,255,0.10)',
                      backgroundColor: 'rgba(255,255,255,0.04)',
                    }}
                  >
                    {call.agent}
                  </span>
                  <span className="truncate font-mono text-[11px] text-zinc-500">
                    {call.model}
                  </span>
                </div>
                <div className="flex shrink-0 items-baseline gap-2 tabular-nums">
                  <span className="text-[12px] font-semibold text-zinc-100">
                    ${call.cost.toFixed(6)}
                  </span>
                  <span className="text-[10px] text-zinc-600">{call.timestamp}</span>
                </div>
              </div>

              {call.where && (
                <div className="text-[12px] leading-relaxed text-zinc-400">
                  {call.where}
                </div>
              )}

              <div className="flex flex-col" style={{ gap: '7px' }}>
                <div className="flex items-center justify-between gap-3 text-[11px] text-zinc-500 tabular-nums">
                  <span>
                    <span className="text-zinc-300">{hits.toLocaleString()}</span>{' '}
                    cached ({hitPercent.toFixed(0)}%)
                    <span className="mx-1.5 text-zinc-700">·</span>
                    <span className="text-zinc-400">{misses.toLocaleString()}</span>{' '}
                    uncached
                  </span>
                  <span>{call.output.toLocaleString()} out</span>
                </div>
                <div
                  className="flex h-[4px] w-full overflow-hidden rounded-full"
                  style={{ backgroundColor: 'rgba(255,255,255,0.07)' }}
                >
                  <div
                    className="h-full rounded-full bg-zinc-200"
                    style={{ width: `${hitPercent}%` }}
                  />
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
