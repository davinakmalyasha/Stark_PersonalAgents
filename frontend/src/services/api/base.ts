export const API_BASE = '/api';

export async function fetchWithRetry(url: string, options?: RequestInit, maxRetries = 8, baseDelayMs = 500): Promise<Response> {
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      const res = await fetch(url, options);
      if (res.ok) return res;
      if (res.status === 502 || res.status === 504 || res.status === 503) {
        throw new Error(`Proxy error: HTTP ${res.status} (backend not ready)`);
      }
      return res;
    } catch (err) {
      if (attempt === maxRetries) throw err;
      const delay = baseDelayMs * Math.pow(2, attempt);
      const message = err instanceof Error ? err.message : String(err);
      console.log(`[api] Retry ${attempt + 1}/${maxRetries} for ${url} — waiting ${delay}ms (${message})`);
      await new Promise(r => setTimeout(r, delay));
    }
  }
  throw new Error('fetchWithRetry: unreachable');
}
