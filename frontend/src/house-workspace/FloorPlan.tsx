import type { HouseDevice, HouseRoom } from './house.types';

// Room layout in plan coordinates (px). Compact apartment footprint.
const LAYOUT: Record<string, { x: number; y: number; w: number; h: number }> = {
  living: { x: 4, y: 4, w: 210, h: 148 },
  kitchen: { x: 218, y: 4, w: 148, h: 96 },
  bathroom: { x: 218, y: 104, w: 148, h: 92 },
  bedroom: { x: 4, y: 156, w: 176, h: 112 },
  garage: { x: 184, y: 156, w: 182, h: 112 },
};

interface Props {
  rooms: HouseRoom[];
  devices: HouseDevice[];
  selectedRoomId: string | null;
  onSelectRoom: (roomId: string) => void;
}

export default function FloorPlan({ rooms, devices, selectedRoomId, onSelectRoom }: Props) {
  const devicesByRoom = new Map<string, HouseDevice[]>();
  for (const d of devices) {
    const list = devicesByRoom.get(d.roomId) ?? [];
    list.push(d);
    devicesByRoom.set(d.roomId, list);
  }

  return (
    <svg viewBox="0 0 370 272" className="w-full h-auto" role="img" aria-label="House floor plan">
      <defs>
        <radialGradient id="hwPlanGlow" cx="50%" cy="42%" r="65%">
          <stop offset="0%" stopColor="rgba(255,255,255,0.05)" />
          <stop offset="100%" stopColor="rgba(255,255,255,0)" />
        </radialGradient>
      </defs>
      <rect x="0" y="0" width="370" height="272" rx="12" fill="url(#hwPlanGlow)" />
      {rooms.map((room) => {
        const box = LAYOUT[room.id];
        if (!box) return null;
        const roomDevices = devicesByRoom.get(room.id) ?? [];
        const isActive = room.id === selectedRoomId;
        return (
          <g key={room.id}>
            <rect
              x={box.x}
              y={box.y}
              width={box.w}
              height={box.h}
              rx="10"
              className={`hw-plan-room ${isActive ? 'is-active' : ''}`}
              onClick={() => onSelectRoom(room.id)}
            />
            <text x={box.x + 12} y={box.y + 20} className="hw-plan-label">
              {room.name}
            </text>
            {roomDevices.slice(0, 12).map((d, i) => {
              const cols = 6;
              const col = i % cols;
              const row = Math.floor(i / cols);
              const cx = box.x + 18 + col * ((box.w - 30) / (cols - 1));
              const cy = box.y + box.h - 14 - row * 13;
              const active =
                (d.type === 'light' && d.state.power) ||
                (d.type === 'plug' && d.state.power) ||
                (d.type === 'camera' && d.state.recording) ||
                d.type === 'thermostat';
              return (
                <circle
                  key={d.id}
                  cx={cx}
                  cy={cy}
                  r={active ? 3.4 : 2.4}
                  className="hw-plan-dot"
                  fill={d.isOnline === false ? 'rgba(255,255,255,0.15)' : active ? '#ffffff' : 'rgba(255,255,255,0.3)'}
                >
                  <title>{`${d.name} — ${d.type}`}</title>
                </circle>
              );
            })}
          </g>
        );
      })}
    </svg>
  );
}
