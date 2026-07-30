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
    <div className="token-analytics flex flex-col" style={{ gap: '20px' }}>
      {/* 1. Header & Summary Grid */}
      <TokenSummary tokenUsage={tokenUsage} lastModelName={lastModelName} />

      {/* 2. Efficiency & Caching Insights */}
      <EfficiencyInsights tokenUsage={tokenUsage} />

      {/* 3. Call Log Details */}
      <CallList calls={tokenUsage.calls} />
    </div>
  );
}
