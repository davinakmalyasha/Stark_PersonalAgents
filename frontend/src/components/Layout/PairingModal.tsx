import { useEffect, useState } from 'react';
import { Copy, Check, RefreshCw, Smartphone, X } from 'lucide-react';
import { QRCodeSVG } from 'qrcode.react';

interface Props {
  open: boolean;
  onClose: () => void;
}

/**
 * Desktop-side pairing panel (mobile/remote feature).
 * Shows the pairing token as text, QR code and one-click link so the phone can
 * pair with this machine over Tailscale/LAN. Only reachable from loopback.
 */
export default function PairingModal({ open, onClose }: Props) {
  const [token, setToken] = useState('');
  const [error, setError] = useState('');
  const [copied, setCopied] = useState<'link' | 'token' | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void (async () => {
      try {
        const res = await fetch('/api/pairing');
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const data = await res.json();
        if (!cancelled) setToken(data.token ?? '');
      } catch {
        if (!cancelled) setError('Could not read the pairing token (local only).');
      }
    })();
    return () => { cancelled = true; };
  }, [open]);

  const pairUrl = token ? `${window.location.origin}/?token=${token}` : '';

  const copy = async (text: string, kind: 'link' | 'token') => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(kind);
      window.setTimeout(() => setCopied(null), 1500);
    } catch { /* clipboard blocked */ }
  };

  const rotate = async () => {
    try {
      const res = await fetch('/api/pairing/rotate', { method: 'POST' });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data = await res.json();
      setToken(data.token ?? '');
    } catch {
      setError('Rotate failed.');
    }
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={onClose}>
      <div
        className="w-full max-w-md rounded-2xl border border-zinc-800 bg-[#111114] shadow-2xl overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-4 border-b border-zinc-800/70">
          <div className="flex items-center gap-2.5">
            <Smartphone size={18} className="text-sky-400" />
            <h2 className="text-sm font-semibold text-zinc-100">Pair your phone</h2>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-lg hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors">
            <X size={16} />
          </button>
        </div>

        <div className="px-5 py-5 space-y-4">
          <p className="text-xs leading-relaxed text-zinc-400">
            Open this server's address on your phone (same Tailscale network or Wi-Fi) and enter
            the token below — or scan / open the link directly.
          </p>

          {error && (
            <div className="rounded-lg border border-red-900/50 bg-red-950/40 px-3 py-2 text-xs text-red-300">{error}</div>
          )}

          {token && (
            <>
              <div className="flex justify-center py-2">
                <div className="rounded-xl bg-white p-3">
                  <QRCodeSVG value={pairUrl} size={168} level="M" />
                </div>
              </div>

              <div className="space-y-2">
                <div className="flex items-center gap-2">
                  <code className="flex-1 truncate rounded-lg border border-zinc-800 bg-black/40 px-3 py-2 text-sm tracking-widest text-zinc-200">{token}</code>
                  <button
                    onClick={() => copy(token, 'token')}
                    className="shrink-0 flex items-center gap-1.5 rounded-lg border border-zinc-800 px-2.5 py-2 text-xs text-zinc-300 hover:bg-zinc-800 transition-colors"
                  >
                    {copied === 'token' ? <Check size={13} className="text-emerald-400" /> : <Copy size={13} />}
                    Token
                  </button>
                </div>
                <div className="flex items-center gap-2">
                  <code className="flex-1 truncate rounded-lg border border-zinc-800 bg-black/40 px-3 py-2 text-xs text-zinc-400">{pairUrl}</code>
                  <button
                    onClick={() => copy(pairUrl, 'link')}
                    className="shrink-0 flex items-center gap-1.5 rounded-lg border border-zinc-800 px-2.5 py-2 text-xs text-zinc-300 hover:bg-zinc-800 transition-colors"
                  >
                    {copied === 'link' ? <Check size={13} className="text-emerald-400" /> : <Copy size={13} />}
                    Link
                  </button>
                </div>
              </div>
            </>
          )}

          <div className="flex items-center justify-between pt-1">
            <button
              onClick={rotate}
              className="flex items-center gap-1.5 text-xs text-zinc-500 hover:text-zinc-300 transition-colors"
              title="Invalidate the current token and generate a new one"
            >
              <RefreshCw size={12} />
              New token
            </button>
            <span className="text-[11px] text-zinc-600">Local devices connect without a token.</span>
          </div>
        </div>
      </div>
    </div>
  );
}
