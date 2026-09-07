// Barrel — re-exports from modular api/* (Lane B split)
// Keeps `import { fetchChats } from '../services/api'` working while new code can `import from '../services/api/chat'`
export * from './api/base';
export * from './api/workspace';
export * from './api/chat';
export * from './api/tasks';
export * from './api/stream';
