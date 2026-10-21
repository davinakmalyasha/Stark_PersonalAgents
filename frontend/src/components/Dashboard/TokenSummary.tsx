import type { TokenUsage } from '../../types';

interface TokenSummaryProps {
  tokenUsage: TokenUsage;
  lastModelName: string;
}

function getModelContextLimit(modelName: string): number {
  const name = (modelName || '').toLowerCase();
  if (name.includes('gemini-1.5-pro') || name.includes('gemini-3.5-pro')) return 2_000_000;
  if (name.includes('gemini') || name.includes('deepseek-v4') || name.includes('deepseek-flash') || name.includes('deepseek-chat') || name.includes('deepseek-v3')) return 1_000_000;
  if (name.includes('claude-3') || name.includes('claude-3-5')) return 200_000;
  if (name.includes('reasoner') || name.includes('r1') || name.includes('gpt-4') || name.includes('llama')) return 128_000;
  return 1_000_000;
}

function formatCompact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 100_000 ? 0 : 1)}K`;
  return `${n}`;
}

export function TokenSummary({ tokenUsage, lastModelName }: TokenSummaryProps) {
  const totalCost = tokenUsage.total_cost ?? 0;
  const input = tokenUsage.total_input_tokens || 0;
  const output = tokenUsage.total_output_tokens || 0;
  const cacheHits = tokenUsage.total_cache_hit_tokens || 0;
  const cacheMisses =
    typeof tokenUsage.total_cache_miss_tokens === 'number'
      ? tokenUsage.total_cache_miss_tokens
      : Math.max(input - cacheHits, 0);
  const totalTokens = input + output;
  const callCount = tokenUsage.calls.length;

  const hitRatio = input > 0 ? (cacheHits / input) * 100 : 0;
  const missRatio = input > 0 ? 100 - hitRatio : 0;

  const maxContextTokens =
    callCount > 0 ? Math.max(...tokenUsage.calls.map((c) => c.input || 0)) : 0;
  const contextLimit = getModelContextLimit(lastModelName);
  const contextPercent = contextLimit > 0 ? (maxContextTokens / contextLimit) * 100 : 0;

  const stats = [
    {
      label: 'Input tokens',
      value: input.toLocaleString(),
      sub: `${formatCompact(input)} total`,
    },
    {
      label: 'Output tokens',
      value: output.toLocaleString(),
      sub: `${formatCompact(output)} total`,
    },
    {
      label: 'Cache hit',
      value: `${hitRatio.toFixed(1)}%`,
      sub: `${cacheHits.toLocaleString()} tokens`,
    },
    {
      label: 'Cache miss',
      value: `${missRatio.toFixed(1)}%`,
      sub: `${cacheMisses.toLocaleString()} tokens`,
    },
  ];

  return (
    <section
      className="flex flex-col"
      style={{
        borderRadius: '14px',
        border: '1px solid rgba(255,255,255,0.08)',
        backgroundColor: 'rgba(255,255,255,0.02)',
      }}
    >
      {/* Header */}
      <div
        className="flex items-start justify-between gap-4"
        style={{ padding: '18px 20px 16px' }}
      >
        <div className="min-w-0">
          <div className="text-[11px] font-medium uppercase tracking-[0.12em] text-zinc-500">
            Total cost
          </div>
          <div className="mt-1.5 flex items-baseline gap-2">
            <span className="text-[28px] leading-none font-semibold tracking-tight text-zinc-50 tabular-nums">
              ${totalCost.toFixed(5)}
            </span>
            <span className="text-[11px] font-medium text-zinc-500">USD</span>
          </div>
        </div>
        <div className="shrink-0 pt-1 text-right text-[11px] leading-relaxed text-zinc-500 tabular-nums">
          <div>{callCount} {callCount === 1 ? 'call' : 'calls'}</div>
          <div>{totalTokens.toLocaleString()} tokens total</div>
        </div>
      </div>

      <div style={{ borderTop: '1px solid rgba(255,255,255,0.07)', margin: '0 20px' }} />

      {/* Stat grid */}
      <div
        className="grid grid-cols-2 sm:grid-cols-4"
        style={{ padding: '14px 20px 6px', gap: '16px' }}
      >
        {stats.map((s) => (
          <div key={s.label} className="min-w-0">
            <div className="text-[11px] font-medium text-zinc-500">{s.label}</div>
            <div className="mt-1 truncate text-[15px] font-semibold text-zinc-100 tabular-nums">
              {s.value}
            </div>
            <div className="mt-0.5 truncate text-[11px] text-zinc-500 tabular-nums">
              {s.sub}
            </div>
          </div>
        ))}
      </div>

      {/* Cache distribution */}
      <div style={{ padding: '12px 20px 0' }}>
        <div
          className="flex h-[5px] w-full overflow-hidden rounded-full"
          style={{ backgroundColor: 'rgba(255,255,255,0.07)' }}
        >
          <div
            className="h-full rounded-full bg-zinc-100 transition-all duration-500"
            style={{ width: `${hitRatio}%` }}
          />
        </div>
        <div className="mt-2 flex items-center justify-between text-[11px] text-zinc-500 tabular-nums">
          <span>{hitRatio.toFixed(1)}% hits</span>
          <span>{missRatio.toFixed(1)}% misses</span>
        </div>
      </div>

      <div style={{ borderTop: '1px solid rgba(255,255,255,0.07)', margin: '14px 20px 0' }} />

      {/* Context footer */}
      <div
        className="flex items-center justify-between gap-4"
        style={{ padding: '12px 20px 16px' }}
      >
        <span className="text-[11px] text-zinc-500">
          Peak context{' '}
          <strong className="font-medium text-zinc-300 tabular-nums">
            {maxContextTokens.toLocaleString()}
          </strong>{' '}
          / {formatCompact(contextLimit)} ({contextPercent.toFixed(1)}%)
        </span>
        <div
          className="h-[3px] w-28 shrink-0 overflow-hidden rounded-full"
          style={{ backgroundColor: 'rgba(255,255,255,0.08)' }}
        >
          <div
            className="h-full rounded-full bg-zinc-300"
            style={{ width: `${Math.min(contextPercent, 100)}%` }}
          />
        </div>
      </div>
    </section>
  );
}
