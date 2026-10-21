import { useEffect, useState } from 'react';
import { Eye } from 'lucide-react';
import { fetchCanvasStatus } from '../../services/api';

export default function CanvasModeBadge({ chatId }: { chatId: string | null }) {
  const [active, setActive] = useState(false);

  useEffect(() => {
    if (!chatId) return;
    let cancelled = false;
    const load = () => {
      fetchCanvasStatus(chatId)
        .then((info) => { if (!cancelled) setActive(Boolean(info.active)); })
        .catch(() => {});
    };
    load();
    const timer = window.setInterval(load, 8000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [chatId]);

  if (!active || !chatId) return null;
  return (
    <span
      className="flex h-8 items-center gap-1.5 rounded-xl border border-amber-400/25 bg-amber-400/10 px-2 text-[10px] font-semibold uppercase tracking-wider text-amber-200"
      title="The orchestrator is watching selected agent chats"
    >
      <Eye size={12} />
      Oversight
    </span>
  );
}
