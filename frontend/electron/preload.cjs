const { contextBridge, ipcRenderer } = require('electron');

// Voice listeners are tracked so re-registering never leaks: every on* returns
// an unsubscribe function and each channel keeps its own set of callbacks.
const _voiceSessionCbs = new Set();
const _voiceClosedCbs = new Set();

function _subscribe(channel, set, cb) {
  const wrapped = (_event, payload) => cb(payload);
  set.add(wrapped);
  ipcRenderer.on(channel, wrapped);
  return () => {
    if (set.delete(wrapped)) {
      ipcRenderer.removeListener(channel, wrapped);
    }
  };
}

contextBridge.exposeInMainWorld('antigravity', {
  selectFolder: () => ipcRenderer.invoke('select-folder'),
  revealPath: (target) => ipcRenderer.invoke('reveal-path', target),
  voiceToggle: () => ipcRenderer.invoke('voice-toggle'),
  voiceText: (text) => ipcRenderer.send('voice-text', text),
  voiceStatus: (status) => ipcRenderer.send('voice-status', status),
  onVoiceWindowClosed: (cb) => _subscribe('voice-window-closed', _voiceClosedCbs, cb),
  onVoiceSessionEvent: (cb) => _subscribe('voice-session-event', _voiceSessionCbs, cb),
  offVoiceSessionEvent: () => {
    _voiceSessionCbs.forEach((w) => ipcRenderer.removeListener('voice-session-event', w));
    _voiceSessionCbs.clear();
  },
  offVoiceWindowClosed: () => {
    _voiceClosedCbs.forEach((w) => ipcRenderer.removeListener('voice-window-closed', w));
    _voiceClosedCbs.clear();
  },
  setVoiceChat: (payload) => ipcRenderer.send('voice-set-chat', payload),
});