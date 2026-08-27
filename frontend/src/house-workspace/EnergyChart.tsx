import type { HouseEnergy } from './house.types';

interface Props {
  energy: HouseEnergy;
}

export default function EnergyChart({ energy }: Props) {
  const series = energy.series.length ? energy.series : [0, 0];
  const w = 220;
  const h = 44;
  const max = Math.max(...series, 0.1);
  const step = w / (series.length - 1 || 1);
  const pts = series.map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * (h - 4) - 2).toFixed(1)}`);
  const line = `M ${pts.join(' L ')}`;
  const area = `${line} L ${w},${h} L 0,${h} Z`;

  return (
    <div>
      <div className="flex items-baseline justify-between">
        <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-600">Energy today</span>
        <span className="text-[10px] text-zinc-500 tabular-nums">${energy.cost.toFixed(2)}</span>
      </div>
      <div className="flex items-end gap-3 mt-1">
        <span className="hw-stat-value text-[24px] font-bold text-white leading-none">
          {energy.todayKwh.toFixed(1)}
          <span className="text-[11px] font-medium text-zinc-500 ml-1">kWh</span>
        </span>
        <svg viewBox={`0 0 ${w} ${h}`} className="flex-1 h-11 min-w-0" preserveAspectRatio="none" aria-hidden="true">
          <defs>
            <linearGradient id="hwSparkGrad" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="rgba(255,255,255,0.22)" />
              <stop offset="100%" stopColor="rgba(255,255,255,0)" />
            </linearGradient>
          </defs>
          <path d={area} className="hw-spark-fill" />
          <path d={line} className="hw-spark-line" vectorEffect="non-scaling-stroke" />
        </svg>
      </div>
    </div>
  );
}
