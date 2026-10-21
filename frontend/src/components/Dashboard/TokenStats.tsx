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
      <div className="p-4 rounded-lg text-xs italic bg-zinc-900/40 border border-zinc-800 text-zinc-400">
        No token usage recorded for this session yet. Start the agentic pipeline to analyze cost.
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
