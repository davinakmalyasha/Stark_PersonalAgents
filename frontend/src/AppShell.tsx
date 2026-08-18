import { useEffect, useRef, useState } from 'react';
import { Code2 } from 'lucide-react';
import App from './App';
import House from './house/House';
import HomeOrb from './house/HomeOrb';
import HousePortal from './house/HousePortal';

type Route = 'home' | 'coding'

function getRoute(): Route {
  const hash = window.location.hash
  if (hash.startsWith('#/coding')) return 'coding'
  return 'home'
}

/**
 * Top-level shell: the House (global assistant home) is the root.
 * The Coding Workspace (the existing app, untouched) is a room
 * entered via `#/coding`. Entering shows the white portal ring
 * transition while the workspace boots.
 */
export default function AppShell() {
  const [route, setRoute] = useState<Route>(getRoute)
  const [portalVisible, setPortalVisible] = useState(false)
  const [portalLeaving, setPortalLeaving] = useState(false)
  const prevRoute = useRef<Route>(getRoute())

  useEffect(() => {
    const onHash = () => setRoute(getRoute())
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [])

  // Show the white circular portal while the coding workspace mounts
  useEffect(() => {
    const prev = prevRoute.current
    prevRoute.current = route
    if (route === 'coding' && prev !== 'coding') {
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
  return <House />
}
