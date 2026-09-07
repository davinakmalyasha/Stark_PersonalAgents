import { Code2, Palette } from 'lucide-react';
import { MODULES, type ModuleId } from './modules';

interface ModuleTrayProps {
  /** Called with the launched module id; the caller owns navigation. */
  onLaunchModule: (id: ModuleId) => void;
  /** Module ids to show. Defaults to all registered modules. */
  include?: ModuleId[];
  /** 'row' (horizontal) or 'column' (vertical rail). */
  direction?: 'row' | 'column';
}

const ICONS = {
  code: Code2,
  palette: Palette,
} as const;

/**
 * Icon rail for workspace modules (IDE, OpenDesign, …).
 * Hovering an icon shows the module's detail text via the native tooltip.
 */
export default function ModuleTray({ onLaunchModule, include, direction = 'row' }: ModuleTrayProps) {
  const defs = include ? MODULES.filter((m) => include.includes(m.id)) : MODULES;
  return (
    <div
      className={`flex shrink-0 items-center gap-1.5 ${
        direction === 'column' ? 'flex-col py-2' : 'flex-row px-1'
      }`}
      role="toolbar"
      aria-label="Workspace modules"
    >
      {defs.map((m) => {
        const Icon = ICONS[m.icon];
        return (
          <button
            key={m.id}
            type="button"
            onClick={() => onLaunchModule(m.id)}
            title={m.detail}
            aria-label={m.label}
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer transition-colors"
          >
            <Icon size={15} />
          </button>
        );
      })}
    </div>
  );
}
