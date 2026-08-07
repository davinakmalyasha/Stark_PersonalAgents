import type { TokenUsage } from '../../types';

interface TokenSummaryProps {
  tokenUsage: TokenUsage;
  lastModelName: string;
}

function getModelContextLimit(modelName: string): number {
  const name = modelName.toLowerCase();
  if (name.includes('gemini-1.5-pro') || name.includes('gemini-3.5-pro')) return 2_000_000;
  if (name.includes('gemini') || name.includes('deepseek-v4') || name.includes('deepseek-chat') || name.includes('deepseek-v3')) return 1_000_000;
  if (name.includes('claude-3') || name.includes('claude-3-5')) return 200_000;
  if (name.includes('reasoner') || name.includes('r1') || name.includes('gpt-4') || name.includes('llama')) return 128_000;
  return 1_000_000;
}

function getModelSavingsRate(modelName: string): number {
  const name = modelName.toLowerCase();
  if (name.includes('deepseek-v4-flash') || name.includes('deepseek-chat') || name.includes('deepseek-v3') || name.includes('deepseek-reasoner')) {
    return 0.1372; // 0.14 - 0.0028
  }
  if (name.includes('deepseek-v4-pro')) {
    return 0.431375; // 0.435 - 0.003625
  }
  if (name.includes('deepseek-r1')) {
    return 0.41; // 0.55 - 0.14
  }
  if (name.includes('gemini-1.5-pro') || name.includes('gemini-3.5-pro')) {
    return 1.25; // 100% discount for cached input
  }
  if (name.includes('gemini-2.0-flash-lite') || name.includes('gemini-3.1-flash-lite')) {
    return 0.0375;
  }
  if (name.includes('gemini')) {
    return 0.075;
  }
  if (name.includes('gpt-4o-mini')) {
    return 0.075; // 50% discount
  }
  if (name.includes('gpt-4o')) {
    return 1.25; // 50% discount
  }
  return 0.126; // fallback default
}

export function TokenSummary({ tokenUsage, lastModelName }: TokenSummaryProps) {
  const totalCost = tokenUsage.total_cost;
  const input = tokenUsage.total_input_tokens || 0;
  const output = tokenUsage.total_output_tokens || 0;
  const cacheHits = tokenUsage.total_cache_hit_tokens || 0;

  // Peak context window across all calls in this chat
  const maxContextTokens = tokenUsage.calls.length > 0
    ? Math.max(...tokenUsage.calls.map((c) => c.input))
    : 0;
  const contextLimit = getModelContextLimit(lastModelName);
  const contextPercent = contextLimit > 0 ? (maxContextTokens / contextLimit) * 100 : 0;
  const contextColor =
    contextPercent >= 85 ? 'text-red-400' :
    contextPercent >= 60 ? 'text-yellow-400' :
    'text-emerald-400';
  const contextBarColor =
    contextPercent >= 85 ? 'bg-red-500 shadow-[0_0_10px_rgba(239,68,68,0.5)]' :
    contextPercent >= 60 ? 'bg-yellow-500 shadow-[0_0_10px_rgba(234,179,8,0.4)]' :
    'bg-emerald-500 shadow-[0_0_10px_rgba(16,185,129,0.5)]';
  const contextStatus =
    contextPercent >= 85 ? 'Critical' :
    contextPercent >= 60 ? 'Elevated' :
    'Healthy';
  const contextBadgeColor =
    contextPercent >= 85 ? 'text-red-400 bg-red-500/10 border-red-500/20' :
    contextPercent >= 60 ? 'text-yellow-400 bg-yellow-500/10 border-yellow-500/20' :
    'text-emerald-400 bg-emerald-500/10 border-emerald-500/20';

  // Calculate Cache Hit Ratio
  const hitRatio = input > 0 ? (cacheHits / input) * 100 : 0;

  // Cache efficiency dynamic colors (same thresholds as pill)
  const cacheColor =
    hitRatio >= 70 ? 'text-emerald-400' :
    hitRatio >= 40 ? 'text-yellow-400' :
    input === 0    ? 'text-zinc-500'   :
    'text-red-400';
  const cacheBarColor =
    hitRatio >= 70 ? 'bg-emerald-500 shadow-[0_0_10px_rgba(16,185,129,0.5)]' :
    hitRatio >= 40 ? 'bg-yellow-500 shadow-[0_0_10px_rgba(234,179,8,0.4)]' :
    input === 0    ? 'bg-zinc-600' :
    'bg-red-500 shadow-[0_0_10px_rgba(239,68,68,0.5)]';
  const cacheStatus =
    hitRatio >= 70 ? 'Good' :
    hitRatio >= 40 ? 'Fair' :
    input === 0    ? 'No data' :
    'Poor';
  const cacheBadgeColor =
    hitRatio >= 70 ? 'text-emerald-400 bg-emerald-500/10 border-emerald-500/20' :
    hitRatio >= 40 ? 'text-yellow-400 bg-yellow-500/10 border-yellow-500/20' :
    input === 0    ? 'text-zinc-500 bg-zinc-800/40 border-zinc-700/30' :
    'text-red-400 bg-red-500/10 border-red-500/20';

  // Calculate Savings from Cache Hits
  let totalSavings = 0;
  for (const call of tokenUsage.calls) {
    const hits = call.cache_hits || 0;
    if (hits <= 0) continue;
    const savingsRate = getModelSavingsRate(call.model);
    totalSavings += (hits / 1000000) * savingsRate;
  }

  const savingsPct = (totalCost + totalSavings) > 0 ? (totalSavings / (totalCost + totalSavings)) * 100 : 0;

  return (
    <div className="grid grid-cols-1 md:grid-cols-2" style={{ gap: '12px' }}>
      {/* Total Cost Card */}
      <div 
        className="hover:bg-zinc-900/50 hover:border-zinc-700/60 transition-all duration-300 flex flex-col justify-between"
        style={{
          padding: '16px',
          borderRadius: '12px',
          border: '1px solid rgba(255, 255, 255, 0.08)',
          backgroundColor: 'rgba(255, 255, 255, 0.02)'
        }}
      >
        <div>
          <span className="text-[10px] uppercase tracking-widest font-bold text-zinc-400 opacity-80">Total Burned</span>
          <div className="mt-3 flex items-baseline gap-1.5">
            <span className="text-2xl font-black bg-gradient-to-r from-emerald-400 to-teal-300 bg-clip-text text-transparent">
              ${totalCost.toFixed(5)}
            </span>
            <span className="text-[9px] font-bold text-zinc-500 uppercase tracking-wider">USD</span>
          </div>
        </div>
        <div 
          className="mt-4 pt-3 flex justify-between text-[10px] text-zinc-400 font-medium"
          style={{ borderTop: '1px solid rgba(255, 255, 255, 0.08)' }}
        >
          <span>In: <strong className="text-zinc-300 font-semibold">{input.toLocaleString()}</strong></span>
          <span>Out: <strong className="text-zinc-300 font-semibold">{output.toLocaleString()}</strong></span>
        </div>
      </div>

      {/* Cache Hit Ratio Card */}
      <div 
        className="hover:bg-zinc-900/50 hover:border-zinc-700/60 transition-all duration-300 flex flex-col justify-between"
        style={{
          padding: '16px',
          borderRadius: '12px',
          border: '1px solid rgba(255, 255, 255, 0.08)',
          backgroundColor: 'rgba(255, 255, 255, 0.02)'
        }}
      >
        <div>
          <span className="text-[10px] uppercase tracking-widest font-bold text-zinc-400 opacity-80">Cache Efficiency</span>
          <div className="mt-3 flex items-baseline gap-2">
            <span className={`text-2xl font-black ${cacheColor}`}>
              {hitRatio.toFixed(1)}%
            </span>
            <span className="text-[10px] font-semibold text-zinc-500">hit rate</span>
          </div>
        </div>
        <div className="mt-4">
          <div 
            className="w-full h-2 rounded-full overflow-hidden border"
            style={{ 
              backgroundColor: 'rgba(255, 255, 255, 0.04)',
              borderColor: 'rgba(255, 255, 255, 0.06)'
            }}
          >
            <div className={`h-full rounded-full transition-all duration-500 ${cacheBarColor}`}
                 style={{ width: `${hitRatio}%` }} />
          </div>
          <div className="flex justify-between items-center mt-2">
            <span className="text-[10px] text-zinc-400 font-medium">
              Cached: <strong className="text-zinc-300 font-semibold">{cacheHits.toLocaleString()}</strong> tokens
            </span>
            <span className={`text-[9px] font-bold uppercase tracking-wider px-1.5 py-0.5 rounded border ${cacheBadgeColor}`}>{cacheStatus}</span>
          </div>
        </div>
      </div>

      {/* Savings Card */}
      <div 
        className="hover:bg-zinc-900/50 hover:border-zinc-700/60 transition-all duration-300 flex flex-col justify-between"
        style={{
          padding: '16px',
          borderRadius: '12px',
          border: '1px solid rgba(255, 255, 255, 0.08)',
          backgroundColor: 'rgba(255, 255, 255, 0.02)'
        }}
      >
        <div>
          <span className="text-[10px] uppercase tracking-widest font-bold text-zinc-400 opacity-80">Estimated Savings</span>
          <div className="mt-3 flex items-baseline gap-1.5">
            <span className="text-2xl font-black bg-gradient-to-r from-teal-300 to-emerald-400 bg-clip-text text-transparent">
              ${totalSavings.toFixed(5)}
            </span>
            <span className="text-[10px] text-emerald-400 font-bold bg-emerald-500/10 px-1.5 py-0.5 rounded border border-emerald-500/20">
              {savingsPct.toFixed(0)}% saved
            </span>
          </div>
        </div>
        <span 
          className="text-[10px] text-zinc-400 font-medium mt-4 pt-3 block"
          style={{ borderTop: '1px solid rgba(255, 255, 255, 0.08)' }}
        >
          Via DeepSeek Context Caching
        </span>
      </div>

      {/* Context Window Card */}
      <div
        className="hover:bg-zinc-900/50 hover:border-zinc-700/60 transition-all duration-300 flex flex-col justify-between"
        style={{
          padding: '16px',
          borderRadius: '12px',
          border: '1px solid rgba(255, 255, 255, 0.08)',
          backgroundColor: 'rgba(255, 255, 255, 0.02)'
        }}
      >
        <div>
          <span className="text-[10px] uppercase tracking-widest font-bold text-zinc-400 opacity-80">Context Window</span>
          <div className="mt-3 flex items-baseline gap-1.5 flex-wrap">
            <span className={`text-2xl font-black ${contextColor}`}>
              {maxContextTokens.toLocaleString()}
            </span>
            <span className="text-[11px] font-semibold text-zinc-500">
              / {contextLimit >= 1_000_000
                ? `${(contextLimit / 1_000_000).toFixed(0)}M`
                : `${(contextLimit / 1_000).toFixed(0)}K`}
            </span>
            <span className={`text-[13px] font-bold ${contextColor}`}>
              ({contextPercent.toFixed(1)}%)
            </span>
          </div>
        </div>
        <div className="mt-4">
          <div
            className="w-full h-2 rounded-full overflow-hidden border"
            style={{
              backgroundColor: 'rgba(255, 255, 255, 0.04)',
              borderColor: 'rgba(255, 255, 255, 0.06)'
            }}
          >
            <div
              className={`h-full rounded-full transition-all duration-500 ${contextBarColor}`}
              style={{ width: `${Math.min(contextPercent, 100)}%` }}
            />
          </div>
          <div className="flex justify-between items-center mt-2">
            <span className="text-[10px] text-zinc-400 font-medium">
              Peak this chat: <strong className="text-zinc-300 font-semibold">{maxContextTokens.toLocaleString()}</strong> tokens
            </span>
            <span className={`text-[9px] font-bold uppercase tracking-wider px-1.5 py-0.5 rounded border ${contextBadgeColor}`}>{contextStatus}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
