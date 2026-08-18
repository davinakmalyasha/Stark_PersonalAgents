import { useMemo, useState } from 'react';
import { ChevronDown, ChevronRight } from 'lucide-react';

interface Props {
  diff: string;
  filename?: string;
}

interface DiffLine {
  type: 'add' | 'del' | 'ctx' | 'hunk' | 'meta';
  oldNo: number | null;
  newNo: number | null;
  text: string;
}

const MAX_VISIBLE_LINES = 800;

/** Parse a unified diff (git-style) into renderable lines with numbers. */
function parseDiff(raw: string): { lines: DiffLine[]; header: string; stats: { add: number; del: number } } {
  const lines: DiffLine[] = [];
  let header = '';
  let add = 0;
  let del = 0;
  let oldNo = 0;
  let newNo = 0;

  for (const rawLine of raw.split('\n')) {
    if (rawLine.startsWith('diff --git') || rawLine.startsWith('index ') || rawLine.startsWith('--- ') || rawLine.startsWith('+++ ')) {
      if (rawLine.startsWith('--- a/') || rawLine.startsWith('+++ b/')) {
        const name = rawLine.slice(6);
        if (name && name !== '/dev/null') header = name;
      }
      lines.push({ type: 'meta', oldNo: null, newNo: null, text: rawLine });
      continue;
    }
    const hunk = rawLine.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/);
    if (hunk) {
      oldNo = parseInt(hunk[1], 10);
      newNo = parseInt(hunk[2], 10);
      lines.push({ type: 'hunk', oldNo: null, newNo: null, text: rawLine });
      continue;
    }
    if (rawLine.startsWith('+')) {
      lines.push({ type: 'add', oldNo: null, newNo: newNo++, text: rawLine });
      add += 1;
    } else if (rawLine.startsWith('-')) {
      lines.push({ type: 'del', oldNo: oldNo++, newNo: null, text: rawLine });
      del += 1;
    } else if (rawLine.startsWith('\\')) {
      lines.push({ type: 'ctx', oldNo: null, newNo: null, text: rawLine });
    } else {
      lines.push({ type: 'ctx', oldNo: oldNo++, newNo: newNo++, text: rawLine });
    }
  }
  return { lines, header, stats: { add, del } };
}

const TYPE_STYLE: Record<DiffLine['type'], string> = {
  add: 'bg-emerald-500/[0.08] text-emerald-200/90',
  del: 'bg-red-500/[0.08] text-red-300/90',
  ctx: 'text-zinc-400',
  hunk: 'bg-indigo-500/[0.06] text-indigo-300/90',
  meta: 'text-zinc-600',
};

export default function DiffView({ diff, filename }: Props) {
  const [truncated, setTruncated] = useState(false);
  const { lines, header, stats } = useMemo(() => parseDiff(diff || ''), [diff]);

  const showTruncate = lines.length > MAX_VISIBLE_LINES;
  const visibleLines = showTruncate && !truncated ? lines.slice(0, MAX_VISIBLE_LINES) : lines;

  if (lines.length === 0) return <div className="text-[12.5px] text-zinc-600">No text diff available for this file.</div>;

  return (
    <div className="rounded-lg border border-zinc-800 bg-zinc-950/60 overflow-hidden">
      <div className="flex items-center justify-between px-3 py-2 border-b border-zinc-800 bg-zinc-900/50">
        <span className="text-[12px] font-mono text-zinc-300 truncate">{filename || header || 'diff'}</span>
        <span className="flex items-center gap-2 flex-shrink-0 text-[11px] font-mono">
          <b className="text-emerald-400">+{stats.add}</b>
          <b className="text-red-400">-{stats.del}</b>
        </span>
      </div>
      <div className="max-h-[420px] overflow-auto">
        <table className="w-full border-collapse font-mono text-[12px] leading-[1.55]">
          <tbody>
            {visibleLines.map((line, i) => (
              <tr key={i} className={TYPE_STYLE[line.type]}>
                <td className="text-right px-2.5 py-0 select-none text-zinc-600 w-[3.5em] border-r border-zinc-800/50">
                  {line.oldNo ?? ''}
                </td>
                <td className="text-right px-2.5 py-0 select-none text-zinc-600 w-[3.5em] border-r border-zinc-800/50">
                  {line.newNo ?? ''}
                </td>
                <td className="px-2.5 py-0 whitespace-pre-wrap break-all">{line.text || ' '}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {showTruncate && (
          <button
            type="button"
            onClick={() => setTruncated((v) => !v)}
            className="w-full flex items-center justify-center gap-1.5 py-2 text-[11.5px] font-semibold text-indigo-400 hover:text-indigo-300 bg-zinc-900/40 border-t border-zinc-800 cursor-pointer"
          >
            {truncated ? <ChevronRight size={13} /> : <ChevronDown size={13} />}
            {truncated ? 'Collapse' : `Show all ${lines.length} lines`}
          </button>
        )}
      </div>
    </div>
  );
}
