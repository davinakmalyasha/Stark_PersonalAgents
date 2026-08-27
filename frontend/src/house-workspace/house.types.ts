// ── House Workspace types ──

export type DeviceType = 'light' | 'thermostat' | 'lock' | 'plug' | 'blinds' | 'camera' | 'sensor';

export interface DeviceState {
  power?: boolean;
  brightness?: number;
  colorTemp?: number;
  targetTemp?: number;
  currentTemp?: number;
  mode?: 'off' | 'heat' | 'cool' | 'auto';
  locked?: boolean;
  position?: number;
  recording?: boolean;
  temperature?: number;
  humidity?: number;
  motion?: boolean;
  smoke?: boolean;
}

export interface HouseDevice {
  id: string;
  name: string;
  roomId: string;
  type: DeviceType;
  state: DeviceState;
  capabilities: string[];
  isOnline: boolean;
  updatedAt: string;
}

export interface HouseRoom {
  id: string;
  name: string;
  icon: string;
}

export interface HouseScene {
  id: string;
  name: string;
  icon: string;
  actions: { deviceId: string; state: Record<string, unknown> }[];
}

export interface HouseEnergy {
  todayKwh: number;
  cost: number;
  series: number[];
}

export interface HouseSecurity {
  armed: boolean;
  mode: string;
  lastEvent: string;
}

export interface HouseState {
  version: number;
  updatedAt: string;
  rooms: HouseRoom[];
  devices: HouseDevice[];
  scenes: HouseScene[];
  energy: HouseEnergy;
  security: HouseSecurity;
}
