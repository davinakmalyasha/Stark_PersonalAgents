import { Sunrise, Moon, Shield, Tv, Target } from 'lucide-react';
import type { HouseScene } from './house.types';

const SCENE_ICON: Record<string, typeof Sunrise> = {
  sunrise: Sunrise,
  moon: Moon,
  shield: Shield,
  tv: Tv,
  target: Target,
};

interface Props {
  scenes: HouseScene[];
  busy: boolean;
  onRun: (sceneId: string) => Promise<void>;
}

export default function SceneBar({ scenes, busy, onRun }: Props) {
  return (
    <div className="flex flex-wrap gap-2">
      {scenes.map((scene) => {
        const Icon = SCENE_ICON[scene.icon] ?? Target;
        return (
          <button
            key={scene.id}
            type="button"
            disabled={busy}
            onClick={() => onRun(scene.id).catch(() => {})}
            className="hw-scene-chip flex items-center gap-2 rounded-xl px-3.5 py-2.5 text-[12px] font-medium text-zinc-200 disabled:cursor-wait"
            title={`Run scene: ${scene.name}`}
          >
            <Icon size={14} className="text-zinc-300" />
            {scene.name}
          </button>
        );
      })}
    </div>
  );
}
