import { Home } from 'lucide-react';

interface Props {
  label: string;
  icon?: React.ReactNode;
  leaving?: boolean;
}

/**
 * Shared portal transition overlay — the white circular core with
 * expanding rings, used for workspace transitions.
 */
export default function HousePortal({ label, icon, leaving }: Props) {
  return (
    <div className={`house-portal ${leaving ? 'is-leaving' : 'is-entering'}`}>
      <div className="flex flex-col items-center gap-5">
        <div className="relative grid place-items-center">
          <div className="house-portal-ring absolute" />
          <div className="house-portal-ring absolute" style={{ animationDelay: '0.35s' }} />
          <div className="grid place-items-center w-16 h-16 rounded-full bg-white text-[#0a0a0d] shadow-[0_0_60px_rgba(255,255,255,0.35)]">
            {icon ?? <Home size={22} />}
          </div>
        </div>
        <span className="house-font-display text-[12px] uppercase tracking-[0.3em] text-zinc-400">{label}</span>
      </div>
    </div>
  );
}
