import { useEffect, useRef, useState } from 'react';
import { Code2, House as HouseIcon, User } from 'lucide-react';
import App from './App';
import House from './house/House';
import HomeOrb from './house/HomeOrb';
import HousePortal from './house/HousePortal';
import HouseWorkspace from './house-workspace/HouseWorkspace';
import PersonalWorkspace from './personal-workspace/PersonalWorkspace';
import PairingGate from './components/Layout/PairingGate';
import { onAuthRejected } from './services/remoteAuth';

type Route = 'home' | 'coding' | 'house' | 'personal'

function getRoute(): Route {
  const hash = window.location.hash
  if (hash.startsWith('#/coding')) return 'coding'
  if (hash.startsWith('#/house')) return 'house'
  if (hash.startsWith('#/personal')) return 'personal'
  return 'home'
}

/**
 * Top-level shell: the House (global assistant home) is the root.
 * The Coding Workspace (the existing app, untouched) is a room
 * entered via `#/coding`. The House Workspace (smart home control)
 * is a room entered via `#/house`. The Personal Workspace is a room
 * entered via `#/personal` (reserved shell). Entering shows the white
 * portal ring transition while the workspace boots.
 */
export default function AppShell() {
  const [route, setRoute] = useState<Route>(getRoute)
  const [portalVisible, setPortalVisible] = useState(false)
  const [portalLeaving, setPortalLeaving] = useState(false)
  const [needsPairing, setNeedsPairing] = useState(false)
  const prevRoute = useRef<Route>(getRoute())

  // Remote access: when the server rejects us (401) show the pairing gate.
  useEffect(() => onAuthRejected(() => setNeedsPairing(true)), [])

  useEffect(() => {
    const onHash = () => setRoute(getRoute())
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  // Show the white circular portal while a workspace mounts
  useEffect(() => {
    const prev = prevRoute.current
    prevRoute.current = route
    if (route !== 'home' && prev !== route) {
      setPortalVisible(true)
      setPortalLeaving(false)
      const t1 = window.setTimeout(() => setPortalLeaving(true), 1400)
      const t2 = window.setTimeout(() => setPortalVisible(false), 1750)
      return () => {
        window.clearTimeout(t1)
        window.clearTimeout(t2)
      }
    }
    return undefined
  }, [route])

  if (needsPairing) {
    return <PairingGate />
  }
  if (route === 'coding') {
    return (
      <>
        <App />
        <HomeOrb />
        {portalVisible && (
          <HousePortal
            label="Entering Coding Workspace"
            icon={<Code2 size={22} />}
            leaving={portalLeaving}
          />
        )}
      </>
    )
  }
  if (route === 'house') {
    return (
      <>
        <HouseWorkspace />
        {portalVisible && (
          <HousePortal
            label="Entering House Workspace"
            icon={<HouseIcon size={22} />}
            leaving={portalLeaving}
          />
        )}
      </>
    )
  }
  if (route === 'personal') {
    return (
      <>
        <PersonalWorkspace />
        {portalVisible && (
          <HousePortal
            label="Entering Personal Workspace"
            icon={<User size={22} />}
            leaving={portalLeaving}
          />
        )}
      </>
    )
  }
  return <House />
}
