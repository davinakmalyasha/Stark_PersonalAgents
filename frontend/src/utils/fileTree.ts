// Shared workspace file-tree builder (used by FileExplorer + CodingIDE).

import type { WorkspaceFileEntry } from '../services/api';

export interface TreeNode {
  name: string;
  path: string;
  type: 'dir' | 'file';
  size?: number;
  children: TreeNode[];
}

export function buildTree(entries: WorkspaceFileEntry[]): TreeNode[] {
  const root: TreeNode[] = [];
  const map = new Map<string, TreeNode>();
  for (const e of entries) {
    const node: TreeNode = { name: e.name, path: e.path, type: e.type, size: e.size, children: [] };
    map.set(e.path, node);
    const parentPath = e.path.includes('/') ? e.path.slice(0, e.path.lastIndexOf('/')) : '';
    const parent = parentPath ? map.get(parentPath) : null;
    if (parent) parent.children.push(node);
    else root.push(node);
  }
  return root;
}
