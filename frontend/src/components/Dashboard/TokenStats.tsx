import type { TokenUsage } from '../../types';
import { TokenSummary } from './TokenSummary';
import { EfficiencyInsights } from './EfficiencyInsights';
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
    <div className="token-analytics grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_minmax(360px,0.9fr)] items-start" style={{ gap: '18px' }}>
      {/* Summary and insights stay together in the left column. */}
      <div className="flex min-w-0 flex-col" style={{ gap: '14px' }}>
        <TokenSummary tokenUsage={tokenUsage} lastModelName={lastModelName} />
        <EfficiencyInsights tokenUsage={tokenUsage} />
      </div>

      {/* The execution log gets its own taller column. */}
      <div className="min-w-0">
        <CallList calls={tokenUsage.calls} />
      </div>
    </div>
  );
}
