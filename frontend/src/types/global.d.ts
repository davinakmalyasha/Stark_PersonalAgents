export {};

declare global {
  interface Window {
    antigravity?: {
      /** Open a native folder picker dialog (Electron). Returns path or null. */
      selectFolder: () => Promise<string | null>;
      /** Toggle the floating voice overlay window (Electron). */
      voiceToggle?: () => Promise<boolean>;
      /** Subscribe to voice session events from the overlay. */
      onVoiceSessionEvent?: (cb: (ev: { type: string; text?: string }) => void) => void;
      /** Fired when the voice overlay window closes. */
      onVoiceWindowClosed?: (cb: () => void) => void;
      /** Tell the overlay which chat is currently active (so live mode
       *  follows the open chat instead of a fixed one). */
      setVoiceChat?: (payload: { wsId: string; wsPath: string; chatId: string }) => void;
    };
    /** Browser File System Access API */
    showDirectoryPicker?: () => Promise<FileSystemDirectoryHandle>;
  }
}
