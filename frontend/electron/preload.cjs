const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('antigravity', {
  selectFolder: () => ipcRenderer.invoke('select-folder'),
  voiceToggle: () => ipcRenderer.invoke('voice-toggle'),
  voiceText: (text) => ipcRenderer.send('voice-text', text),
  voiceStatus: (status) => ipcRenderer.send('voice-status', status),
  onVoiceWindowClosed: (cb) => {
    ipcRenderer.on('voice-window-closed', () => cb());
  },
  onVoiceSessionEvent: (cb) => {
    ipcRenderer.on('voice-session-event', (_event, payload) => cb(payload));
  },
  setVoiceChat: (payload) => ipcRenderer.send('voice-set-chat', payload),
});
