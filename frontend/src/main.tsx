import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import AppShell from './AppShell'
import { installRemoteAuth } from './services/remoteAuth'

installRemoteAuth()

const isTauri = Boolean((window as unknown as { __TAURI__?: unknown }).__TAURI__)
if ('serviceWorker' in navigator) {
  if (isTauri) {
    navigator.serviceWorker.getRegistrations()
      .then((registrations) => Promise.all(registrations.map((registration) => registration.unregister())))
      .catch(() => {})
  } else if (import.meta.env.PROD) {
    // The installed PWA precaches index.html + assets, so a new deploy is
    // invisible until the SW updates. Check on every load, on focus, and
    // every minute; reload once when the new worker takes control.
    let reloading = false
    navigator.serviceWorker.addEventListener('controllerchange', () => {
      if (reloading) return
      reloading = true
      window.location.reload()
    })
    const register = () => {
      navigator.serviceWorker
        .register('/sw.js', { updateViaCache: 'none' })
        .then((registration) => registration.update().catch(() => {}))
        .catch(() => {})
    }
    register()
    window.addEventListener('online', register)
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) register()
    })
    window.setInterval(register, 60_000)
  }
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppShell />
  </StrictMode>,
)
