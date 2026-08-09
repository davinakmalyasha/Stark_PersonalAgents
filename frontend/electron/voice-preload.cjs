const { contextBridge, ipcRenderer } = require('electron');

// Minimal bridge for the floating voice overlay window (voice.html).
contextBridge.exposeInMainWorld('voiceBridge', {
  sendEvent: (payload) => ipcRenderer.send('voice-session-event', payload),
  close: () => ipcRenderer.send('voice-close'),
  resize: (w, h) => ipcRenderer.send('voice-resize', w, h),
  onSetChat: (cb) => {
    ipcRenderer.on('voice-set-chat', (_event, payload) => cb(payload));
  },
});
