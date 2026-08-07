import { TrendingUp, AlertTriangle, Lightbulb, CheckCircle2 } from 'lucide-react';
import type { TokenUsage } from '../../types';

interface EfficiencyInsightsProps {
  tokenUsage: TokenUsage;
}

const AGENT_MAPPING = [
  { keys: ['dev', 'coder'], name: 'developer' },
  { keys: ['ba'], name: 'planning (ba)' },
  { keys: ['sa'], name: 'architecture (sa)' },
  { keys: ['tester', 'qa'], name: 'tester (qa)' },
  { keys: ['devops'], name: 'devops' },
  { keys: ['supervisor', 'director'], name: 'supervisor' },
  { keys: ['analytics'], name: 'analytics' }
];

export function EfficiencyInsights({ tokenUsage }: EfficiencyInsightsProps) {
  const { calls, total_cost: totalCost } = tokenUsage;
  if (calls.length < 2) return null;

  const agentCosts: Record<string, number> = {};
  const agentInputs: Record<string, number> = {};
  const agentHits: Record<string, number> = {};

  for (const call of calls) {
    const rawAgent = call.agent.toLowerCase();
    const match = AGENT_MAPPING.find(m => m.keys.some(k => rawAgent.includes(k)));
    const agent = match ? match.name : 'other';

    agentCosts[agent] = (agentCosts[agent] || 0) + call.cost;
    agentInputs[agent] = (agentInputs[agent] || 0) + call.input;
    agentHits[agent] = (agentHits[agent] || 0) + (call.cache_hits || 0);
  }

  let maxCostAgent = '', maxCost = 0;
  for (const [agent, cost] of Object.entries(agentCosts)) {
    if (cost > maxCost) { maxCost = cost; maxCostAgent = agent; }
  }

  let minRatioAgent = '', minRatio = 100;
  for (const [agent, input] of Object.entries(agentInputs)) {
    if (input > 1000) {
      const ratio = ((agentHits[agent] || 0) / input) * 100;
      if (ratio < minRatio) { minRatio = ratio; minRatioAgent = agent; }
    }
  }

  const costPercentage = totalCost > 0 ? (maxCost / totalCost) * 100 : 0;

  return (
    <div 
      className="text-xs flex flex-col"
      style={{
        padding: '16px',
        borderRadius: '12px',
        border: '1px solid rgba(255, 255, 255, 0.08)',
        backgroundColor: 'rgba(255, 255, 255, 0.02)',
        gap: '12px'
      }}
    >
      <div className="font-bold uppercase tracking-wider flex items-center gap-2 text-indigo-400">
        <TrendingUp size={16} /> Cost &amp; Cache Efficiency Insights
      </div>
      <div className="grid grid-cols-1 lg:grid-cols-2" style={{ gap: '10px' }}>
        {maxCost > 0 && (
          <div 
            className="flex gap-2.5 items-start"
            style={{
              padding: '10px 12px',
              borderLeft: '3px solid #f59e0b',
              backgroundColor: 'rgba(245, 158, 11, 0.04)',
              borderRadius: '0 8px 8px 0'
            }}
          >
            <AlertTriangle size={16} className="text-amber-500 flex-shrink-0" style={{ marginTop: '2px' }} />
            <div>
              <div className="font-bold text-amber-400 tracking-wide mb-0.5">Primary Cost Driver</div>
              <p className="text-zinc-300 leading-relaxed">
                The <strong className="capitalize text-zinc-100 font-semibold">{maxCostAgent}</strong> agent is responsible for{' '}
                <strong className="text-amber-400 font-bold">{costPercentage.toFixed(0)}%</strong> of the total run cost (${maxCost.toFixed(5)}).
              </p>
            </div>
          </div>
        )}
        {minRatioAgent && minRatio < 60 && (
          <div 
            className="flex gap-2.5 items-start"
            style={{
              padding: '10px 12px',
              borderLeft: '3px solid #3b82f6',
              backgroundColor: 'rgba(59, 130, 246, 0.04)',
              borderRadius: '0 8px 8px 0'
            }}
          >
            <Lightbulb size={16} className="text-blue-400 flex-shrink-0" style={{ marginTop: '2px' }} />
            <div>
              <div className="font-bold text-blue-400 tracking-wide mb-0.5">Cache Miss Inefficiency</div>
              <p className="text-zinc-300 leading-relaxed">
                <strong className="capitalize text-zinc-100 font-semibold">{minRatioAgent}</strong> has a low prompt cache hit rate of{' '}
                <strong className="text-red-400 font-bold">{minRatio.toFixed(1)}%</strong>.
                {minRatioAgent === 'developer' && ' Use partial reads (`offset`/`limit`) or `view_signatures` first to reuse prompt prefixes.'}
                {minRatioAgent === 'tester (qa)' && ' Traceback logs are too large. Log compactors/cleaners are critical.'}
              </p>
            </div>
          </div>
        )}
        {(!minRatioAgent || minRatio >= 60) && maxCost > 0 && (
          <div 
            className="flex gap-2.5 items-start"
            style={{
              padding: '10px 12px',
              borderLeft: '3px solid #10b981',
              backgroundColor: 'rgba(16, 185, 129, 0.04)',
              borderRadius: '0 8px 8px 0'
            }}
          >
            <CheckCircle2 size={16} className="text-emerald-400 flex-shrink-0" style={{ marginTop: '2px' }} />
            <div>
              <div className="font-bold text-emerald-400 tracking-wide mb-0.5">Optimal Cache Performance</div>
              <p className="text-zinc-300 leading-relaxed">
                All agents are utilizing DeepSeek KV cache hits efficiently (&gt;60% hit ratio). Prefix reuse is working perfectly.
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
