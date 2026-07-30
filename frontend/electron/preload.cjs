const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('antigravity', {
  selectFolder: () => ipcRenderer.invoke('select-folder'),
  voiceToggle: () => ipcRenderer.invoke('voice-toggle'),
  voiceText: (text) => ipcRenderer.send('voice-text', text),
  voiceStatus: (status) => ipcRenderer.send('voice-status', status),
  onVoiceWindowClosed: (cb) => {
    ipcRenderer.on('voice-window-closed', () => cb());
  },
});
