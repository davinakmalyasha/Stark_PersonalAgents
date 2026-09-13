import { useCallback, useEffect, useState } from 'react';
import { Activity, ArrowLeft, RefreshCw, Server, ShieldCheck, Terminal, Trash2 } from 'lucide-react';
import { fetchAdminHealth, fetchBots, fetchClients, fetchUsage, disableBot } from './agentsAdminApi';

interface TextBoxProps {
  title: string;
  body?: string | null;
  error?: string | null;
}

function TextBox({ title, body, error }: TextBoxProps) {
  return (
    <section className="hw-panel rounded-2xl p-4">
      <h3 className="flex items-center gap-2 text-[12px] font-bold uppercase tracking-widest text-zinc-400">
        <Terminal size={13} /> {title}
      </h3>
      {error ? (
        <p className="mt-2.5 text-[12.5px] text-red-400 whitespace-pre-wrap break-words">{error}</p>
      ) : body ? (
        <pre className="mt-2.5 text-[12px] leading-relaxed text-zinc-300 whitespace-pre-wrap break-words font-mono">{body}</pre>
      ) : (
        <p className="mt-2.5 text-[12px] text-zinc-600">Belum ada data / No data yet.</p>
      )}
    </section>
  );
}

export default function AgentsAdminWorkspace() {
  const [busy, setBusy] = useState<string | null>(null);
  const [health, setHealth] = useState<{ text?: string; error?: string } | null>(null);
  const [clients, setClients] = useState<{ text?: string; error?: string } | null>(null);
  const [bots, setBots] = useState<{ text?: string; error?: string } | null>(null);
  const [usage, setUsage] = useState<{ text?: string; error?: string } | null>(null);
  const [tenantFilter, setTenantFilter] = useState('');

  const run = useCallback(async (label: string, fn: () => Promise<{ ok: boolean; text?: string; error?: string }>) => {
    setBusy(label);
    try {
      const r = await fn();
      if (label === 'health') setHealth(r);
      else if (label === 'clients') setClients(r);
      else if (label === 'bots') setBots(r);
      else if (label === 'usage') setUsage(r);
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    void run('health', fetchAdminHealth);
    void run('clients', fetchClients);
  }, [run]);

  const refreshAll = () => {
    void run('health', fetchAdminHealth);
    void run('clients', fetchClients);
    void run('bots', () => fetchBots(tenantFilter));
  };

  return (
    <div className="hw-root fixed inset-0 flex flex-col overflow-hidden">
      <header className="flex items-center justify-between px-4 sm:px-6 h-[52px] border-b house-hairline shrink-0">
        <div className="flex items-center gap-2.5">
          <button
            type="button"
            onClick={() => { window.location.hash = ''; }}
            title="Back to home"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="grid place-items-center w-7 h-7 rounded-lg bg-white text-[#0a0a0d]">
            <ShieldCheck size={14} />
          </div>
          <span className="house-font-display text-[15px] font-bold tracking-wide text-white">Agents Admin</span>
          <span className="text-[10px] uppercase tracking-widest text-zinc-600 border border-white/10 rounded-full px-2 py-0.5">
            super-operator
          </span>
        </div>
        <button
          type="button"
          onClick={refreshAll}
          disabled={!!busy}
          title="Refresh"
          className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer disabled:opacity-40"
        >
          <RefreshCw size={15} className={busy ? 'animate-spin' : ''} />
        </button>
      </header>

      <main className="flex-1 min-h-0 overflow-y-auto px-4 sm:px-6 py-5 space-y-4">
        <div className="grid gap-4 lg:grid-cols-2">
          <TextBox title="Health" body={health?.text} error={health?.error} />
          <TextBox title="Clients" body={clients?.text} error={clients?.error} />
        </div>

        <section className="hw-panel rounded-2xl p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="flex items-center gap-2 text-[12px] font-bold uppercase tracking-widest text-zinc-400">
              <Server size={13} /> Bots
            </h3>
            <div className="flex items-center gap-2">
              <input
                value={tenantFilter}
                onChange={(e) => setTenantFilter(e.target.value)}
                placeholder="filter tenant (kosong = semua)"
                className="rounded-lg bg-white/[0.06] border border-white/10 px-2.5 py-1.5 text-[12px] text-zinc-200 outline-none focus:border-white/25 w-44"
              />
              <button
                type="button"
                disabled={!!busy}
                onClick={() => run('bots', () => fetchBots(tenantFilter))}
                className="rounded-lg bg-white text-[#0a0a0d] px-3 py-1.5 text-[12px] font-semibold disabled:opacity-40"
              >
                Load
              </button>
            </div>
          </div>
          {bots?.error ? (
            <p className="mt-2.5 text-[12.5px] text-red-400">{bots.error}</p>
          ) : bots?.text ? (
            <pre className="mt-2.5 text-[12px] leading-relaxed text-zinc-300 whitespace-pre-wrap break-words font-mono">{bots.text}</pre>
          ) : (
            <p className="mt-2.5 text-[12px] text-zinc-600">Load bots to inspect the fleet.</p>
          )}
        </section>

        <div className="grid gap-4 lg:grid-cols-2">
          <section className="hw-panel rounded-2xl p-4">
            <h3 className="flex items-center gap-2 text-[12px] font-bold uppercase tracking-widest text-zinc-400">
              <Activity size={13} /> Tenant usage
            </h3>
            <div className="mt-2.5 flex gap-2">
              <input
                value={tenantFilter}
                onChange={(e) => setTenantFilter(e.target.value)}
                placeholder="tenant_id"
                className="rounded-lg bg-white/[0.06] border border-white/10 px-2.5 py-1.5 text-[12px] text-zinc-200 outline-none focus:border-white/25 flex-1"
              />
              <button
                type="button"
                disabled={!!busy || !tenantFilter.trim()}
                onClick={() => run('usage', () => fetchUsage(tenantFilter.trim()))}
                className="rounded-lg bg-white text-[#0a0a0d] px-3 py-1.5 text-[12px] font-semibold disabled:opacity-40"
              >
                Load
              </button>
            </div>
            {usage?.error ? (
              <p className="mt-2.5 text-[12.5px] text-red-400">{usage.error}</p>
            ) : usage?.text ? (
              <pre className="mt-2.5 text-[12px] leading-relaxed text-zinc-300 whitespace-pre-wrap break-words font-mono">{usage.text}</pre>
            ) : null}
          </section>

          <section className="hw-panel rounded-2xl p-4">
            <h3 className="flex items-center gap-2 text-[12px] font-bold uppercase tracking-widest text-zinc-400">
              <Trash2 size={13} /> Disable a bot (break-glass)
            </h3>
            <div className="mt-2.5 flex gap-2">
              <input
                id="bot-id-input"
                placeholder="bot UUID"
                className="rounded-lg bg-white/[0.06] border border-white/10 px-2.5 py-1.5 text-[12px] text-zinc-200 outline-none focus:border-white/25 flex-1"
              />
              <button
                type="button"
                disabled={!!busy}
                onClick={async () => {
                  const id = (document.getElementById('bot-id-input') as HTMLInputElement | null)?.value?.trim();
                  if (!id) return;
                  const r = await disableBot(id);
                  setBots(r.ok ? { text: `${r.text ?? ''}\n(bot disabled)` } : { error: r.error ?? 'failed' });
                  void run('bots', () => fetchBots(tenantFilter));
                }}
                className="rounded-lg bg-red-500/90 text-white px-3 py-1.5 text-[12px] font-semibold disabled:opacity-40"
              >
                Disable
              </button>
            </div>
          </section>
        </div>
      </main>
    </div>
  );
}
