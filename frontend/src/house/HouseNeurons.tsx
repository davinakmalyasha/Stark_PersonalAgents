import { useEffect, useRef } from 'react';

interface Node {
  angle: number;
  radiusX: number;
  radiusY: number;
  speed: number;
  phase: number;
  size: number;
  alpha: number;
}

/**
 * Monochrome neuron constellation: a pulsing core with orbiting nodes
 * connected by drifting lines. Pure canvas 2D, zero dependencies.
 * Respects prefers-reduced-motion (renders one static frame).
 */
export default function HouseNeurons({ className }: { className?: string }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

    let width = 0;
    let height = 0;
    let raf = 0;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);

    const nodes: Node[] = [
      { angle: 0, radiusX: 0.22, radiusY: 0.22, speed: 0.00042, phase: 0.0, size: 2.2, alpha: 0.55 },
      { angle: 0, radiusX: 0.34, radiusY: 0.3, speed: -0.0003, phase: 1.4, size: 1.8, alpha: 0.45 },
      { angle: 0, radiusX: 0.46, radiusY: 0.38, speed: 0.00024, phase: 2.9, size: 1.5, alpha: 0.4 },
      { angle: 0, radiusX: 0.58, radiusY: 0.45, speed: -0.00019, phase: 4.1, size: 1.3, alpha: 0.34 },
      { angle: 0, radiusX: 0.72, radiusY: 0.52, speed: 0.00015, phase: 5.5, size: 1.2, alpha: 0.3 },
      { angle: 0, radiusX: 0.85, radiusY: 0.58, speed: -0.00012, phase: 0.8, size: 1.1, alpha: 0.26 },
      { angle: 0, radiusX: 0.3, radiusY: 0.4, speed: 0.0005, phase: 2.0, size: 1.6, alpha: 0.42 },
      { angle: 0, radiusX: 0.5, radiusY: 0.36, speed: -0.00036, phase: 3.6, size: 1.4, alpha: 0.36 },
      { angle: 0, radiusX: 0.66, radiusY: 0.5, speed: 0.00028, phase: 6.2, size: 1.2, alpha: 0.3 },
      { angle: 0, radiusX: 0.8, radiusY: 0.55, speed: -0.00022, phase: 7.4, size: 1.1, alpha: 0.26 },
      { angle: 0, radiusX: 0.94, radiusY: 0.62, speed: 0.0001, phase: 9.1, size: 1.0, alpha: 0.22 },
      { angle: 0, radiusX: 0.4, radiusY: 0.28, speed: -0.00044, phase: 4.8, size: 1.5, alpha: 0.4 },
      { angle: 0, radiusX: 0.6, radiusY: 0.44, speed: 0.00032, phase: 0.5, size: 1.3, alpha: 0.32 },
      { angle: 0, radiusX: 0.9, radiusY: 0.6, speed: -0.00014, phase: 5.0, size: 1.0, alpha: 0.24 },
    ];

    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      width = rect.width;
      height = rect.height;
      canvas.width = Math.max(1, Math.floor(width * dpr));
      canvas.height = Math.max(1, Math.floor(height * dpr));
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };

    const draw = (time: number) => {
      ctx.clearRect(0, 0, width, height);
      const cx = width / 2;
      const cy = height / 2;
      const base = Math.min(width, height);
      const pulse = 0.75 + 0.25 * Math.sin(time * 0.0012);

      // Orbiting nodes
      const pts: { x: number; y: number; r: number; a: number }[] = [];
      for (const n of nodes) {
        const ang = n.angle + time * n.speed + n.phase;
        const x = cx + Math.cos(ang) * n.radiusX * base * 0.62;
        const y = cy + Math.sin(ang) * n.radiusY * base * 0.62;
        pts.push({ x, y, r: n.size * pulse, a: n.alpha });
      }

      // Lines: core → node, and node → nearest neighbor
      ctx.lineWidth = 1;
      for (let i = 0; i < pts.length; i++) {
        const p = pts[i];
        ctx.strokeStyle = `rgba(255,255,255,${p.a * 0.16})`;
        ctx.beginPath();
        ctx.moveTo(cx, cy);
        ctx.lineTo(p.x, p.y);
        ctx.stroke();
      }
      for (let i = 0; i < pts.length; i++) {
        const p = pts[i];
        let nearest = -1;
        let nearestD = Infinity;
        for (let j = 0; j < pts.length; j++) {
          if (j === i) continue;
          const d = (p.x - pts[j].x) ** 2 + (p.y - pts[j].y) ** 2;
          if (d < nearestD) { nearestD = d; nearest = j; }
        }
        if (nearest >= 0) {
          const q = pts[nearest];
          const alpha = Math.max(0, 0.1 - nearestD / (base * base * 0.35)) * pulse;
          if (alpha > 0.008) {
            ctx.strokeStyle = `rgba(255,255,255,${alpha})`;
            ctx.beginPath();
            ctx.moveTo(p.x, p.y);
            ctx.lineTo(q.x, q.y);
            ctx.stroke();
          }
        }
      }

      // Core: halo + solid nucleus
      const halo = ctx.createRadialGradient(cx, cy, 0, cx, cy, base * 0.16 * pulse);
      halo.addColorStop(0, 'rgba(255,255,255,0.16)');
      halo.addColorStop(0.55, 'rgba(255,255,255,0.045)');
      halo.addColorStop(1, 'rgba(255,255,255,0)');
      ctx.fillStyle = halo;
      ctx.beginPath();
      ctx.arc(cx, cy, base * 0.16 * pulse, 0, Math.PI * 2);
      ctx.fill();

      ctx.fillStyle = `rgba(250,250,250,${0.85 * pulse})`;
      ctx.beginPath();
      ctx.arc(cx, cy, 3.2, 0, Math.PI * 2);
      ctx.fill();

      ctx.fillStyle = 'rgba(255,255,255,0.35)';
      ctx.beginPath();
      ctx.arc(cx, cy, 1.1, 0, Math.PI * 2);
      ctx.fill();

      // Nodes
      for (const p of pts) {
        ctx.fillStyle = `rgba(255,255,255,${p.a})`;
        ctx.beginPath();
        ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
        ctx.fill();
      }
    };

    const loop = (now: number) => {
      draw(now);
      raf = requestAnimationFrame(loop);
    };

    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);

    if (reduced) {
      draw(0);
    } else {
      raf = requestAnimationFrame(loop);
    }

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  return <canvas ref={canvasRef} className={className} aria-hidden="true" />;
}
