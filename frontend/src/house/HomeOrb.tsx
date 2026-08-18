import { useCallback, useEffect, useState } from 'react';
import { Home } from 'lucide-react';
import HousePortal from './HousePortal';

/**
 * Floating orb shown inside the Coding Workspace. Returns to the House
 * with a portal wipe. Escape also returns home.
 */
export default function HomeOrb() {
  const [transitioning, setTransitioning] = useState(false);

  const goHome = useCallback(() => {
    if (transitioning) return;
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/';
    }, 300);
  }, [transitioning]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') goHome();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [goHome]);

  return (
    <>
      <button
        onClick={goHome}
        className="house-orb"
        title="Back to Home (Esc)"
        aria-label="Back to Home"
      >
        <Home size={17} />
      </button>
      {transitioning && <HousePortal label="Returning home" />}
    </>
  );
}
