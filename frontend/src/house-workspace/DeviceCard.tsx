import { useState } from 'react';
import {
  Lightbulb, Thermometer, Lock, LockOpen, Plug, Camera, Activity, Blinds,
  Video, VideoOff, Wind, Flame, Snowflake,
} from 'lucide-react';
import type { HouseDevice, DeviceState } from './house.types';

const TYPE_ICON = {
  light: Lightbulb,
  thermostat: Thermometer,
  lock: Lock,
  plug: Plug,
  blinds: Blinds,
  camera: Camera,
  sensor: Activity,
} as const;

interface Props {
  device: HouseDevice;
  busy: boolean;
  onChange: (deviceId: string, state: Partial<DeviceState>) => Promise<void>;
}

export default function DeviceCard({ device, busy, onChange }: Props) {
  const [local, setLocal] = useState<DeviceState>(device.state);
  const [error, setError] = useState<string | null>(null);
  const state = { ...device.state, ...local };
  const Icon = TYPE_ICON[device.type] ?? Activity;

  const apply = async (patch: Partial<DeviceState>) => {
    const prev = local;
    setLocal({ ...state, ...patch }); // optimistic
    setError(null);
    try {
      await onChange(device.id, patch);
    } catch (e) {
      setLocal(prev);
      setError(e instanceof Error ? e.message : 'Failed');
    }
  };

  const isOn =
    state.power === true || state.locked === false || state.recording === true || device.type === 'thermostat';
  const offline = device.isOnline === false;
  const readOnly = device.type === 'sensor' || offline;

  return (
    <div className={`hw-panel hw-device-card rounded-xl p-3.5 flex flex-col gap-2.5 ${isOn ? 'is-on' : 'is-off'}`}>
      <div className="flex items-start gap-2.5">
        <span className={`hw-device-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-300 shrink-0`}>
          <Icon size={15} />
        </span>
        <div className="flex-1 min-w-0">
          <div className="text-[12.5px] font-medium text-zinc-100 truncate leading-tight">{device.name}</div>
          <div className="text-[10px] text-zinc-500 mt-0.5 flex items-center gap-1.5">
            <span className="uppercase tracking-wider">{device.type}</span>
            {offline && <span className="text-zinc-600">· offline</span>}
            {device.type === 'sensor' && state.temperature != null && (
              <span>· {state.temperature.toFixed(1)}°C</span>
            )}
            {device.type === 'sensor' && state.humidity != null && <span>· {state.humidity}% RH</span>}
            {device.type === 'sensor' && state.motion != null && <span>· {state.motion ? 'motion' : 'clear'}</span>}
            {device.type === 'sensor' && state.smoke != null && <span>· {state.smoke ? 'SMOKE' : 'ok'}</span>}
          </div>
        </div>
        {!readOnly && (
          <button
            type="button"
            disabled={busy}
            aria-label={isOn ? 'Turn off' : 'Turn on'}
            onClick={() => {
              if (device.type === 'lock') apply({ locked: !state.locked });
              else if (device.type === 'camera') apply({ recording: !state.recording });
              else if (device.type === 'thermostat') apply({ mode: state.mode === 'off' ? 'auto' : 'off' });
              else apply({ power: !state.power });
            }}
            className={`hw-switch ${isOn ? 'is-on' : ''}`}
          />
        )}
      </div>

      {/* Type-specific controls */}
      {device.type === 'light' && state.power && (
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-zinc-500 w-14 shrink-0">Brightness</span>
            <input
              type="range" min={0} max={100} step={1}
              value={state.brightness ?? 0}
              disabled={busy || readOnly}
              onChange={(e) => setLocal({ ...state, brightness: Number(e.target.value) })}
              onMouseUp={(e) => apply({ brightness: Number((e.target as HTMLInputElement).value) })}
              onTouchEnd={(e) => apply({ brightness: Number((e.target as HTMLInputElement).value) })}
              className="hw-slider"
            />
            <span className="text-[10px] text-zinc-400 w-8 text-right tabular-nums">{state.brightness ?? 0}%</span>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-zinc-500 w-14 shrink-0">Warmth</span>
            <input
              type="range" min={2200} max={6500} step={100}
              value={state.colorTemp ?? 3500}
              disabled={busy || readOnly}
              onChange={(e) => setLocal({ ...state, colorTemp: Number(e.target.value) })}
              onMouseUp={(e) => apply({ colorTemp: Number((e.target as HTMLInputElement).value) })}
              onTouchEnd={(e) => apply({ colorTemp: Number((e.target as HTMLInputElement).value) })}
              className="hw-slider"
            />
            <span className="text-[10px] text-zinc-400 w-8 text-right tabular-nums">{state.colorTemp ?? 3500}K</span>
          </div>
        </div>
      )}

      {device.type === 'blinds' && (
        <div className="flex items-center gap-2">
          <span className="text-[10px] text-zinc-500 w-14 shrink-0">Open</span>
          <input
            type="range" min={0} max={100} step={5}
            value={state.position ?? 0}
            disabled={busy || readOnly}
            onChange={(e) => setLocal({ ...state, position: Number(e.target.value) })}
            onMouseUp={(e) => apply({ position: Number((e.target as HTMLInputElement).value) })}
            onTouchEnd={(e) => apply({ position: Number((e.target as HTMLInputElement).value) })}
            className="hw-slider"
          />
          <span className="text-[10px] text-zinc-400 w-8 text-right tabular-nums">{state.position ?? 0}%</span>
        </div>
      )}

      {device.type === 'lock' && (
        <div className="flex items-center gap-2 text-[11px]">
          {state.locked ? <Lock size={11} className="text-zinc-300" /> : <LockOpen size={11} className="text-zinc-300" />}
          <span className={state.locked ? 'text-zinc-300' : 'text-zinc-100 font-medium'}>
            {state.locked ? 'Locked' : 'Unlocked'}
          </span>
        </div>
      )}

      {device.type === 'camera' && (
        <div className="flex items-center gap-2 text-[11px]">
          {state.recording ? <Video size={11} className="text-zinc-300" /> : <VideoOff size={11} className="text-zinc-500" />}
          <span className={state.recording ? 'text-zinc-300' : 'text-zinc-500'}>{state.recording ? 'Recording' : 'Idle'}</span>
        </div>
      )}

      {device.type === 'thermostat' && (
        <div className="space-y-2.5">
          <div className="flex items-center gap-3">
            <button
              type="button" disabled={busy || readOnly}
              onClick={() => apply({ targetTemp: Math.max(10, (state.targetTemp ?? 22) - 0.5) })}
              className="hw-panel is-hoverable rounded-md w-7 h-7 grid place-items-center text-zinc-300 text-[14px] cursor-pointer"
            >−</button>
            <div className="flex-1 text-center">
              <span className="hw-stat-value text-[22px] font-bold text-white">{(state.targetTemp ?? 22).toFixed(1)}°</span>
              <span className="text-[10px] text-zinc-500 ml-1.5">now {(state.currentTemp ?? 22).toFixed(1)}°</span>
            </div>
            <button
              type="button" disabled={busy || readOnly}
              onClick={() => apply({ targetTemp: Math.min(35, (state.targetTemp ?? 22) + 0.5) })}
              className="hw-panel is-hoverable rounded-md w-7 h-7 grid place-items-center text-zinc-300 text-[14px] cursor-pointer"
            >+</button>
          </div>
          <div className="hw-seg flex text-[10.5px]">
            {(['off', 'heat', 'cool', 'auto'] as const).map((m) => (
              <button
                key={m}
                type="button"
                disabled={busy || readOnly}
                onClick={() => apply({ mode: m })}
                className={`flex-1 py-1 capitalize flex items-center justify-center gap-1 ${state.mode === m ? 'is-active' : ''}`}
              >
                {m === 'heat' && <Flame size={9} />}
                {m === 'cool' && <Snowflake size={9} />}
                {m === 'auto' && <Wind size={9} />}
                {m}
              </button>
            ))}
          </div>
        </div>
      )}

      {device.type === 'plug' && state.power && (
        <div className="text-[11px] text-zinc-400 flex items-center gap-1.5">
          <Plug size={11} />
          <span>Powered</span>
        </div>
      )}

      {error && <div className="text-[10px] text-red-400/90">{error}</div>}
    </div>
  );
}
