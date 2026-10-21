const { contextBridge, ipcRenderer } = require('electron');

// Minimal bridge for the floating voice overlay window (voice.html).
// Expose the API port so voice.html always hits the right backend even if
// STARK_PORT is changed (unique port, no hardcoded 8000).
contextBridge.exposeInMainWorld('voiceBridge', {
  sendEvent: (payload) => ipcRenderer.send('voice-session-event', payload),
  close: () => ipcRenderer.send('voice-close'),
  resize: (w, h, animate) => ipcRenderer.send('voice-resize', w, h, !!animate),
  onSetChat: (cb) => {
    ipcRenderer.on('voice-set-chat', (_event, payload) => cb(payload));
  },
  getStarkPort: () => process.env.STARK_PORT || '8123',
  listScreenSources: () => ipcRenderer.invoke('voice-screen-sources'),
  selectScreenSource: (id) => ipcRenderer.invoke('voice-screen-select', id),
  setScreenProtection: (on) => ipcRenderer.send('voice-screen-protect', !!on),
});
