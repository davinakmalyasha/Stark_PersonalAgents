import { X, Activity } from 'lucide-react';
import { TokenStats } from '../Dashboard/TokenStats';
import type { TokenUsage } from '../../types';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  tokenUsage: TokenUsage;
  lastModelName: string;
}

export default function TokenStatsModal({ isOpen, onClose, tokenUsage, lastModelName }: Props) {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-md p-4 animate-fade-in">
      {/* Modal Card */}
      <div 
        className="modal-panel-enter w-[calc(100vw-220px)] max-w-[1280px] h-[min(620px,calc(100vh-48px))] border bg-zinc-950 flex flex-col overflow-hidden"
        style={{ 
          borderColor: 'var(--border-color)',
          borderRadius: '16px',
          boxShadow: '0 25px 50px -12px rgba(0, 0, 0, 0.5)',
          maxHeight: 'calc(100vh - 12px)'
        }}
      >
        {/* Header with explicit padding */}
        <div 
          className="flex items-center justify-between flex-shrink-0"
          style={{ 
            paddingLeft: '24px',
            paddingRight: '24px',
            paddingTop: '20px',
            paddingBottom: '16px',
            borderBottom: '1px solid rgba(255, 255, 255, 0.08)'
          }}
        >
          <div className="flex items-center gap-3">
            <div 
              className="p-2 rounded-lg text-zinc-300 border"
              style={{ 
                backgroundColor: 'rgba(255,255,255,0.04)',
                borderColor: 'rgba(255,255,255,0.09)'
              }}
            >
              <Activity size={18} />
            </div>
            <div>
              <h2 className="text-sm font-semibold text-zinc-100 tracking-tight">Usage</h2>
              <p className="text-[11px] text-zinc-500 mt-0.5">Cost, context, and cache activity for this chat</p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1.5 text-zinc-400 hover:text-zinc-200 hover:bg-white/5 rounded-md transition-all duration-200 cursor-pointer"
          >
            <X size={16} />
          </button>
        </div>

        {/* Content Area with explicit padding */}
        <div 
          className="flex flex-1 flex-col overflow-y-auto pr-1.5 min-h-0"
          style={{ 
            paddingLeft: '24px',
            paddingRight: '24px',
            paddingTop: '18px',
            paddingBottom: '20px'
          }}
        >
          <TokenStats tokenUsage={tokenUsage} lastModelName={lastModelName} />
        </div>
      </div>
    </div>
  );
}
