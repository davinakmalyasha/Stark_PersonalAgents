import { Activity } from 'lucide-react';
import type { TokenUsage } from '../../types';
import { TokenSummary } from './TokenSummary';
import { CallList } from './CallList';

interface TokenStatsProps {
  tokenUsage: TokenUsage;
  lastModelName: string;
}

export function TokenStats({ tokenUsage, lastModelName }: TokenStatsProps) {
  if (!tokenUsage || !tokenUsage.calls || tokenUsage.calls.length === 0) {
    return (
      <div className="flex min-h-[420px] flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
        <div className="grid h-12 w-12 place-items-center rounded-2xl border border-zinc-800 bg-white/[0.035] text-zinc-400">
          <Activity size={22} />
        </div>
        <div>
          <div className="text-sm font-semibold text-zinc-200">No usage yet</div>
          <p className="mt-1 max-w-sm text-[12px] leading-5 text-zinc-500">
            Run a message to see cost, token usage, cache activity, and context here.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="token-analytics grid grid-cols-1 xl:grid-cols-[minmax(0,1.05fr)_minmax(360px,0.95fr)] items-start" style={{ gap: '16px' }}>
      {/* Overview: totals + input / output / cache hit / cache miss. */}
      <div className="min-w-0">
        <TokenSummary tokenUsage={tokenUsage} lastModelName={lastModelName} />
      </div>

      {/* Per-request breakdown. */}
      <div className="min-w-0">
        <CallList calls={tokenUsage.calls} />
      </div>
    </div>
  );
}
