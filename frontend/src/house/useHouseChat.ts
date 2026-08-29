// SHIM — moved to hooks/useChat.ts (Lane B)
// Keeps `import { useHouseChat } from '../house/useHouseChat'` working
export * from '../hooks/useChat';
export { useChat as useHouseChat } from '../hooks/useChat';
