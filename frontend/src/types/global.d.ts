export {};

declare global {
  interface Window {
    /** Absolute API origin injected by the Tauri shell; empty in web/PWA. */
    __STARK_API_ORIGIN__?: string;
    /** Voice overlay bridge port. */
    __STARK_PORT__?: number | string;
    /** Desktop shell marker (`tauri`). */
    __STARK_DESKTOP__?: string;
    antigravity?: {
      /** Open a native folder picker dialog (desktop shell). Returns path or null. */
      selectFolder: () => Promise<string | null>;
      /** LAN base URL the phone can use to reach this desktop. */
      getPairingBase?: () => Promise<string>;
      /** Reveal a project folder in the OS file manager. The main process
       *  rejects any path outside the projects root. */
      revealPath?: (target: string) => Promise<{ ok: boolean; error?: string }>;
      /** Toggle the floating voice overlay window (desktop shell). */
      voiceToggle?: () => Promise<boolean>;
      /** Subscribe to voice session events from the overlay.
       *  Returns an unsubscribe function (and off*() clears all). */
      onVoiceSessionEvent?: (cb: (ev: {
        type: string;
        text?: string;
        kind?: string;
        agent?: string;
        mic?: boolean;
        workspaceId?: string;
        chatId?: string;
        prompt?: string;
        state?: unknown;
      }) => void) => (() => void) | void;
      /** Fired when the voice overlay window closes.
       *  Returns an unsubscribe function (and off*() clears all). */
      onVoiceWindowClosed?: (cb: () => void) => (() => void) | void;
      /** Remove all voice session event listeners. */
      offVoiceSessionEvent?: () => void;
      /** Remove all voice window-closed listeners. */
      offVoiceWindowClosed?: () => void;
      /** Tell the overlay which chat is currently active (so live mode
       *  follows the open chat instead of a fixed one). */
      setVoiceChat?: (payload: { wsId: string; wsPath: string; chatId: string; model?: string; temperature?: number }) => void;
    };
    /** Browser File System Access API */
    showDirectoryPicker?: () => Promise<FileSystemDirectoryHandle>;
  }
}
