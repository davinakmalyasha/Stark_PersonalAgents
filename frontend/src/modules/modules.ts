/**
 * Workspace modules — launchable add-on surfaces that attach to a workspace.
 *
 * A module is a sibling concept to the IDE: it opens a dedicated surface
 * bound to a workspace + a dedicated agent context, without replacing the
 * workspace itself.
 *
 * - `ide`         → coding surface bound to the Dev agent context.
 * - `open-design` → design addon bound straight to the UI subagent loop
 *                   (workspace_kind="design"). It opens inside the coding
 *                   workspace, not as a separate workspace route.
 */
export type ModuleId = 'open-design';

export interface ModuleDef {
  id: ModuleId;
  /** Short label shown next to the icon. */
  label: string;
  /** One-line detail shown on hover (title tooltip). */
  detail: string;
  /** Lucide icon key rendered by ModuleTray. */
  icon: 'code' | 'palette';
  /** Hash route opened by the module (may carry ?ws=&chat=). */
  route: string;
}

export const MODULES: ModuleDef[] = [
  {
    id: 'open-design',
    label: 'OpenDesign',
    detail:
      'OpenDesign — agent-led UI design iteration for this workspace (straight to the UI subagent loop)',
    icon: 'palette',
    route: '#/design',
  },
];

export function getModule(id: ModuleId): ModuleDef {
  const found = MODULES.find((m) => m.id === id);
  if (!found) throw new Error(`Unknown module: ${id}`);
  return found;
}
