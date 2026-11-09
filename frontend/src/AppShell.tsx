import { useEffect, useState } from 'react';
import House from './house/House';
import PairingGate from './components/Layout/PairingGate';
import UpdateToast from './components/Layout/UpdateToast';
import { onAuthRejected } from './services/remoteAuth';
import { startAutoUpdate } from './utils/appUpdate';

/**
 * Top-level shell: one page. The House (global assistant home) is the app —
 * every chat, panel and overlay lives here. Legacy room links (`#/coding`,
 * `#/house`, `#/personal`, `#/memory`, `#/cloud`, `#/admin`) all land home;
 * `#/design` opens the OpenDesign overlay on home.
 *
 * Desktop auto-update checks on launch and shows the same status toast.
 */
export default function AppShell() {
  const [needsPairing, setNeedsPairing] = useState(false)

  // Remote access: when the server rejects us (401) show the pairing gate.
  useEffect(() => onAuthRejected(() => setNeedsPairing(true)), [])

  // Signed desktop auto-update: applies on launch, restarts when idle.
  useEffect(() => startAutoUpdate(), [])

  return (
    <>
      {needsPairing ? <PairingGate /> : <House />}
      <UpdateToast />
    </>
  )
}
