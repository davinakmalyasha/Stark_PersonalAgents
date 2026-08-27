import { useEffect, useMemo, useState } from 'react';
import { FileText, Folder, FolderOpen, Copy, X, ChevronRight, ChevronDown, Search } from 'lucide-react';
import type { WorkspaceFileEntry } from '../../services/api';
import { fetchWorkspaceFiles, fetchWorkspaceFile } from '../../services/api';
import { buildTree, type TreeNode } from '../../utils/fileTree';

interface Props {
  workspaceId: string | null;
  onClose: () => void;
}

/** P10/Tier-3: workspace file explorer — list tree, click to preview. */
export default function FileExplorer({ workspaceId, onClose }: Props) {
  const [entries, setEntries] = useState<WorkspaceFileEntry[]>([]);
  const [openDirs, setOpenDirs] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<{ text: string; truncated: boolean } | null>(null);
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState('');

  useEffect(() => {
    let cancelled = false;
    fetchWorkspaceFiles(workspaceId ?? '').then((files) => {
      if (!cancelled) setEntries(files);
    });
    return () => { cancelled = true; };
  }, [workspaceId]);

  const tree = useMemo(() => buildTree(entries), [entries]);
  const filteredTree = useMemo(() => {
    if (!filter.trim()) return tree;
    const q = filter.toLowerCase();
    const walk = (nodes: TreeNode[]): TreeNode[] => nodes
      .map((n) => ({ ...n, children: walk(n.children) }))
      .filter((n) => n.name.toLowerCase().includes(q) || n.children.length > 0);
    return walk(tree);
  }, [tree, filter]);

  const openFile = async (path: string) => {
    if (!workspaceId) return;
    setSelected(path);
    setLoading(true);
    const res = await fetchWorkspaceFile(workspaceId, path);
    setLoading(false);
    setContent(res ? { text: res.content, truncated: res.truncated } : null);
  };

  const toggleDir = (path: string) => {
    setOpenDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  };

  const renderNode = (node: TreeNode, depth: number): React.ReactNode => {
    const pad = { paddingLeft: 10 + depth * 14 };
    if (node.type === 'dir') {
      const open = openDirs.has(node.path);
      return (
        <div key={node.path}>
          <button
            type="button"
            onClick={() => toggleDir(node.path)}
            className="w-full flex items-center gap-1.5 px-1.5 py-[3px] text-left text-[12.5px] text-zinc-300 hover:bg-zinc-800/60 rounded cursor-pointer"
            style={pad}
          >
            {open ? <ChevronDown size={12} className="text-zinc-500" /> : <ChevronRight size={12} className="text-zinc-500" />}
            {open ? <FolderOpen size={13} className="text-indigo-400 flex-shrink-0" /> : <Folder size={13} className="text-indigo-400 flex-shrink-0" />}
            <span className="truncate">{node.name}</span>
          </button>
          {open && node.children.map((c) => renderNode(c, depth + 1))}
        </div>
      );
    }
    const isSel = selected === node.path;
    return (
      <button
        key={node.path}
        type="button"
        onClick={() => openFile(node.path)}
        className={`w-full flex items-center gap-1.5 px-1.5 py-[3px] text-left text-[12.5px] rounded cursor-pointer ${isSel ? 'bg-indigo-500/15 text-indigo-200' : 'text-zinc-400 hover:bg-zinc-800/60'}`}
        style={pad}
      >
        <FileText size={12} className="text-zinc-600 flex-shrink-0" />
        <span className="truncate flex-1">{node.name}</span>
        {node.size !== undefined && <span className="text-[10px] text-zinc-600 flex-shrink-0">{(node.size / 1024).toFixed(node.size < 1024 ? 1 : 0)}K</span>}
      </button>
    );
  };

  return (
    <div className="panel-right-enter fixed top-14 right-3 bottom-3 w-[380px] z-[90] flex flex-col rounded-2xl border border-zinc-800/80 bg-[#101013]/95 backdrop-blur-xl shadow-2xl overflow-hidden">
      <div className="flex items-center gap-2 px-3.5 py-3 border-b border-zinc-800">
        <Search size={13} className="text-zinc-500 flex-shrink-0" />
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter files…"
          className="flex-1 bg-transparent text-[12.5px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none"
        />
        <button type="button" onClick={onClose} className="p-1 text-zinc-500 hover:text-zinc-300" aria-label="Close files"><X size={14} /></button>
      </div>

      <div className="flex-1 overflow-y-auto px-2 py-2">
        {!workspaceId && <div className="text-[12px] text-zinc-600 px-3 py-6 text-center">Select a project workspace to browse its files.</div>}
        {workspaceId && filteredTree.length === 0 && <div className="text-[12px] text-zinc-600 px-3 py-6 text-center">No files listed (empty or filtered).</div>}
        {filteredTree.map((n) => renderNode(n, 0))}
      </div>

      <div className="border-t border-zinc-800 flex flex-col min-h-0 flex-1">
        {selected ? (
          <div className="flex flex-col h-full min-h-0">
            <div className="flex items-center gap-1.5 px-3.5 py-2 border-b border-zinc-800 bg-zinc-900/40">
              <FileText size={12} className="text-zinc-500 flex-shrink-0" />
              <span className="flex-1 truncate text-[11.5px] font-mono text-zinc-300">{selected}</span>
              <button
                type="button"
                onClick={() => navigator.clipboard?.writeText(selected)}
                className="p-1 text-zinc-500 hover:text-zinc-300"
                title="Copy path"
              >
                <Copy size={12} />
              </button>
            </div>
            <div className="flex-1 overflow-y-auto px-3.5 py-3 min-h-0">
              {loading ? (
                <div className="text-[12px] text-zinc-600">Loading…</div>
              ) : content ? (
                <>
                  <pre className="text-[12px] text-zinc-300 whitespace-pre-wrap break-all leading-relaxed font-mono">{content.text}</pre>
                  {content.truncated && <div className="text-[11px] text-amber-400/80 mt-2">File truncated at 200 KB — preview only.</div>}
                </>
              ) : (
                <div className="text-[12px] text-red-400">Failed to read file.</div>
              )}
            </div>
          </div>
        ) : (
          <div className="text-[11.5px] text-zinc-600 px-4 py-4 text-center">Select a file to preview it.</div>
        )}
      </div>
    </div>
  );
}
