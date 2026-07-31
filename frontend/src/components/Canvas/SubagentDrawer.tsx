import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import type { ChatMessage } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import TraceStepRow from '../Workspace/TraceStepRow';

interface Props {
  isOpen: boolean;
  subagentName: string | null;
  status: string;
  taskDesc: string;
  steps: TraceStep[];
  workspaceId: string | null;
  chatId: string | null;
  onClose: () => void;
}

export default function SubagentDrawer({ isOpen, subagentName, status, taskDesc, steps, workspaceId, chatId, onClose }: Props) {
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [loading, setLoading] = useState(false);
  const [cachedAgent, setCachedAgent] = useState<string | null>(null);
  const [cachedStatus, setCachedStatus] = useState('idle');
  const [cachedTask, setCachedTask] = useState('');
  const [cachedSteps, setCachedSteps] = useState<TraceStep[]>([]);

  useEffect(() => {
    if (subagentName) {
      setCachedAgent(subagentName);
      setCachedStatus(status);
      setCachedTask(taskDesc);
      setCachedSteps(steps);
    }
  }, [subagentName, status, taskDesc, steps]);

  useEffect(() => {
    if (isOpen && subagentName && workspaceId && chatId) {
      setLoading(true);
      fetch(`/api/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/subagents/${encodeURIComponent(subagentName)}`)
        .then((res) => (res.ok ? res.json() : []))
        .then((data) => setHistory(data))
        .catch(() => setHistory([]))
        .finally(() => setLoading(false));
    }
  }, [isOpen, subagentName, workspaceId, chatId]);

  if (!cachedAgent) return null;

  const filteredSteps = cachedSteps.filter((s) => s.type !== 'thought');

  return (
    <div className={`absolute top-0 right-0 h-full w-[420px] bg-[#101012] border-l border-zinc-800 shadow-2xl flex flex-col z-50 transition-transform duration-300 ease-in-out ${
      isOpen ? 'translate-x-0' : 'translate-x-full'
    }`}>
      <div className="p-4 border-b border-zinc-800 flex items-center justify-between flex-shrink-0">
        <div>
          <h3 className="text-[15px] font-bold text-zinc-100">{cachedAgent} Workspace</h3>
          <span className="text-[11px] text-zinc-500 capitalize">Status: {cachedStatus}</span>
        </div>
        <button onClick={onClose} className="p-1.5 hover:bg-zinc-800 rounded-lg text-zinc-400 hover:text-zinc-200 cursor-pointer">
          <X size={16} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {cachedTask && (
          <div className="text-[13px] text-zinc-300 whitespace-pre-wrap leading-relaxed">
            {cachedTask}
          </div>
        )}

        <div className="space-y-2.5">
          <h4 className="text-[11px] font-bold text-zinc-500 uppercase tracking-wider">Internal execution log ({filteredSteps.length} steps)</h4>
          <div className="space-y-1.5 pl-1 border-l border-zinc-800/60">
            {filteredSteps.map((s, idx) => (
              <TraceStepRow key={s.id || idx} step={s} />
            ))}
          </div>
        </div>

        {loading ? (
          <div className="text-center py-6 text-zinc-500 text-[12px]">Loading internal chat history...</div>
        ) : history.length > 0 ? (
          <div className="space-y-3 pt-2">
            <h4 className="text-[11px] font-bold text-zinc-500 uppercase tracking-wider">LLM Dialogue</h4>
            <div className="space-y-3">
              {history.map((msg, i) => (
                <div key={i} className={`p-2.5 rounded-lg text-[12.5px] border ${
                  msg.role === 'user' ? 'bg-zinc-800/30 border-zinc-700 text-zinc-300' : 'bg-indigo-950/15 border-indigo-900/30 text-indigo-200'
                }`}>
                  <span className="text-[10px] font-semibold block uppercase tracking-wider mb-1 text-zinc-500">{msg.role}</span>
                  <div className="whitespace-pre-wrap leading-relaxed">{msg.content}</div>
                </div>
              ))}
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}
