// ── Remote access auth (mobile/remote feature) ──
// The API server requires an X-API-Token header from non-loopback clients.
// The phone saves its pairing token once (via ?token= link or manual entry);
// every subsequent /api request carries it automatically.

const TOKEN_KEY = 'vincode.pairingToken';

type RejectListener = () => void;
const rejectListeners = new Set<RejectListener>();

export function getPairingToken(): string {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? '';
  } catch {
    return '';
  }
}

export function setPairingToken(token: string): void {
  try {
    localStorage.setItem(TOKEN_KEY, token.trim());
  } catch { /* storage unavailable — session-only usage */ }
}

export function clearPairingToken(): void {
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch { /* ignore */ }
}

/** Called by the fetch wrapper whenever the server answers 401. */
export function onAuthRejected(listener: RejectListener): () => void {
  rejectListeners.add(listener);
  return () => rejectListeners.delete(listener);
}

function notifyRejected(): void {
  for (const fn of rejectListeners) {
    try { fn(); } catch { /* listener errors must not break fetch */ }
  }
}

/**
 * Pull `?token=` / `#...token=` out of the URL, persist it, and clean the
 * address bar so the token is not left lying around in history/share sheets.
 * Returns the captured token (or '').
 */
export function captureUrlToken(): string {
  try {
    const search = new URLSearchParams(window.location.search);
    let token = search.get('token') ?? '';
    if (!token && window.location.hash.includes('token=')) {
      const hashQuery = window.location.hash.split('?')[1] ?? '';
      token = new URLSearchParams(hashQuery).get('token') ?? '';
    }
    if (token) {
      setPairingToken(token);
      // Strip the token back out of the URL.
      search.delete('token');
      const cleanSearch = search.toString();
      const cleanHash = window.location.hash.split('?')[0];
      const url = window.location.pathname
        + (cleanSearch ? `?${cleanSearch}` : '')
        + (cleanHash || '');
      window.history.replaceState(null, '', url);
    }
    return token;
  } catch {
    return '';
  }
}

let installed = false;

/** Wrap window.fetch once so all /api calls carry the pairing token. */
export function installRemoteAuth(): void {
  if (installed) return;
  installed = true;
  captureUrlToken();

  const originalFetch = window.fetch.bind(window);
  window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const token = getPairingToken();
    if (!token) return originalFetch(input, init);

    try {
      const url = typeof input === 'string'
        ? input
        : input instanceof URL ? input.toString() : input.url;
      // Same-origin API calls only — never touch external hosts.
      if (!url.includes('/api/')) return originalFetch(input, init);

      const headers = new Headers(
        init?.headers ?? (typeof input !== 'string' && !(input instanceof URL) ? input.headers : undefined),
      );
      if (!headers.has('X-API-Token')) headers.set('X-API-Token', token);

      const res = await originalFetch(input, { ...init, headers });
      if (res.status === 401) notifyRejected();
      return res;
    } catch {
      return originalFetch(input, init);
    }
  };
}
