import { useEffect, useState } from 'react';
import { Smartphone, KeyRound, ArrowRight } from 'lucide-react';
import { getPairingToken, setPairingToken } from '../../services/remoteAuth';
import { apiUrl } from '../../services/api';

/**
 * Phone-side pairing gate (mobile/remote feature).
 * Rendered when the server answers 401 — the viewer is remote and has not
 * paired yet. Enter the token shown on the desktop (or open the ?token= link)
 * to unlock the app.
 */
export default function PairingGate() {
  const [value, setValue] = useState(getPairingToken());
  const [busy, setBusy] = useState(false);

  // Auto-submit once a URL token was captured on load.
  useEffect(() => {
    if (getPairingToken()) window.location.reload();
  }, []);

  const submit = async () => {
    if (!value.trim() || busy) return;
    setBusy(true);
    setPairingToken(value);
    try {
      const res = await fetch(apiUrl('/api/health'));
      if (res.ok) {
        window.location.reload();
        return;
      }
    } catch { /* fall through */ }
    setBusy(false);
  };

  return (
    <div className="fixed inset-0 z-[200] flex items-center justify-center bg-[#0a0a0c] p-6">
      <div className="w-full max-w-sm space-y-6 text-center">
        <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-2xl border border-sky-900/60 bg-sky-950/40">
          <Smartphone size={26} className="text-sky-400" />
        </div>
        <div className="space-y-2">
          <h1 className="text-lg font-semibold text-zinc-100">Pair with your desktop</h1>
          <p className="text-xs leading-relaxed text-zinc-500">
            This server is reachable but locked. Enter the pairing token from the desktop app
            (phone icon in the title bar), then tap continue.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="relative flex-1">
            <KeyRound size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-zinc-600" />
            <input
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') submit(); }}
              placeholder="pairing token"
              autoFocus
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              className="w-full rounded-xl border border-zinc-800 bg-black/50 py-3 pl-9 pr-3 text-sm tracking-widest text-zinc-100 placeholder:normal-case placeholder:tracking-normal placeholder:text-zinc-600 focus:border-sky-700 focus:outline-none"
            />
          </div>
          <button
            onClick={submit}
            disabled={!value.trim() || busy}
            className="flex h-[46px] w-[46px] shrink-0 items-center justify-center rounded-xl bg-sky-600 text-white transition-opacity hover:bg-sky-500 disabled:opacity-40"
            aria-label="Continue"
          >
            <ArrowRight size={18} />
          </button>
        </div>
      </div>
    </div>
  );
}
