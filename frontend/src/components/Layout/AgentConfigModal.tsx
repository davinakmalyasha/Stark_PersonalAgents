import { useCallback, useEffect, useMemo, useState } from 'react';
import { Bot, X } from 'lucide-react';
import {
  deleteAgentConfig,
  fetchAgentConfigs,
  saveAgentConfig,
  setProvider,
  type AgentConfigEntry,
  type AgentConfigPayload,
  type AgentOverride,
} from '../../services/api';

interface Props {
  isOpen: boolean;
  onClose: () => void;
}

interface FormState {
  provider: string;
  model: string;
  thinking: string;
  tools: string;
  skills: string;
  mcp: string;
  systemPrompt: string;
}

const EMPTY_FORM: FormState = {
  provider: '',
  model: '',
  thinking: '',
  tools: '',
  skills: '',
  mcp: '',
  systemPrompt: '',
};

function linesToValue(values: string[] | null | undefined): string {
  if (values === null || values === undefined) return '';
  if (values.length === 0) return 'none';
  return values.join(', ');
}

function valueToLines(value: string): string[] | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (trimmed.toLowerCase() === 'none' || trimmed === '-') return [];
  return trimmed
    .split(/[,\n]/)
    .map((item) => item.trim())
    .filter(Boolean);
}

function formFor(entry: AgentConfigEntry): FormState {
  const override = entry.override || {};
  return {
    provider: override.provider || '',
    model: override.model || '',
    thinking: override.thinking || '',
    tools: linesToValue(override.tools),
    skills: linesToValue(override.skills),
    mcp: linesToValue(override.mcp),
    systemPrompt: override.system_prompt || '',
  };
}

export default function AgentConfigModal({ isOpen, onClose }: Props) {
  const [payload, setPayload] = useState<AgentConfigPayload | null>(null);
  const [selected, setSelected] = useState<string>('developer');
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [status, setStatus] = useState<string>('');
  const [error, setError] = useState<string>('');
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (keep?: string) => {
    setError('');
    try {
      const next = await fetchAgentConfigs();
      setPayload(next);
      const key = keep && next.agents.some((agent) => agent.key === keep)
        ? keep
        : next.agents.some((agent) => agent.key === selected)
          ? selected
          : next.agents[0]?.key || 'developer';
      setSelected(key);
      const entry = next.agents.find((agent) => agent.key === key);
      if (entry) setForm(formFor(entry));
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : 'Could not load config.');
    }
  }, [selected]);

  useEffect(() => {
    if (isOpen) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  const entry = payload?.agents.find((agent) => agent.key === selected) || null;
  const activeProvider = payload?.active_provider || '';
  const formProvider = form.provider || activeProvider;
  const provider = payload?.providers.find((candidate) => candidate.id === formProvider) || null;
  const models = useMemo(() => provider?.models || [], [provider]);

  const selectAgent = (key: string) => {
    setSelected(key);
    setStatus('');
    setError('');
    const next = payload?.agents.find((agent) => agent.key === key);
    if (next) setForm(formFor(next));
  };

  const save = async () => {
    if (!entry) return;
    setBusy(true);
    setError('');
    setStatus('');
    const override: AgentOverride = {
      provider: form.provider.trim() || null,
      model: form.model.trim() || null,
      thinking: form.thinking.trim() || null,
      tools: valueToLines(form.tools),
      skills: valueToLines(form.skills),
      mcp: valueToLines(form.mcp),
      system_prompt: form.systemPrompt.trim() || null,
    };
    try {
      await saveAgentConfig(entry.key, override);
      await load(entry.key);
      setStatus(`Saved ${entry.display}. The next run uses this config.`);
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : 'Could not save.');
    } finally {
      setBusy(false);
    }
  };

  const reset = async () => {
    if (!entry) return;
    setBusy(true);
    setError('');
    setStatus('');
    try {
      await deleteAgentConfig(entry.key);
      await load(entry.key);
      setStatus(`Reset ${entry.display} to the built-in config.`);
    } catch (resetError) {
      setError(resetError instanceof Error ? resetError.message : 'Could not reset.');
    } finally {
      setBusy(false);
    }
  };

  const switchProvider = async (id: string) => {
    setBusy(true);
    setError('');
    setStatus('');
    try {
      await setProvider(id);
      await load(selected);
      window.dispatchEvent(new CustomEvent('stark-provider-changed'));
      setStatus(`Active provider is now ${id}.`);
    } catch (switchError) {
      setError(switchError instanceof Error ? switchError.message : 'Could not switch provider.');
    } finally {
      setBusy(false);
    }
  };

  if (!isOpen) return null;

  const inputClass = 'w-full rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-[12.5px] text-zinc-200 outline-none focus:border-indigo-400/40';
  const labelClass = 'block text-[10px] font-semibold uppercase tracking-[0.14em] text-zinc-500 mb-1.5';

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-md p-4 animate-fade-in">
      <div
        className="modal-panel-enter border bg-zinc-950 flex flex-col overflow-hidden"
        style={{
          borderColor: 'var(--border-color)',
          borderRadius: '16px',
          width: 'min(980px, calc(100vw - 48px))',
          height: 'min(680px, calc(100vh - 48px))',
          boxShadow: '0 25px 50px -12px rgba(0, 0, 0, 0.5)',
        }}
      >
        <div
          className="flex items-center justify-between flex-shrink-0"
          style={{ padding: '18px 22px', borderBottom: '1px solid rgba(255,255,255,0.08)' }}
        >
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg text-zinc-300 border" style={{ backgroundColor: 'rgba(255,255,255,0.04)', borderColor: 'rgba(255,255,255,0.09)' }}>
              <Bot size={18} />
            </div>
            <div>
              <h2 className="text-sm font-semibold text-zinc-100 tracking-tight">Agents & providers</h2>
              <p className="text-[11px] text-zinc-500 mt-0.5">
                Per-agent provider, model, reasoning effort, tools, skills, and prompt
              </p>
            </div>
          </div>
          <button onClick={onClose} className="p-1.5 text-zinc-400 hover:text-zinc-200 hover:bg-white/5 rounded-md transition-all" aria-label="Close">
            <X size={16} />
          </button>
        </div>

        <div className="flex flex-1 min-h-0">
          <aside className="w-[210px] flex-shrink-0 border-r border-white/[0.07] overflow-y-auto" style={{ padding: '12px' }}>
            <div className="mb-3">
              <label className={labelClass}>Active provider</label>
              <select
                className={inputClass}
                value={activeProvider}
                onChange={(event) => void switchProvider(event.target.value)}
                disabled={busy}
              >
                {(payload?.providers || []).map((candidate) => (
                  <option key={candidate.id} value={candidate.id}>
                    {candidate.label}{candidate.has_key ? '' : ' (no key)'}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-0.5">
              {(payload?.agents || []).map((agent) => (
                <button
                  key={agent.key}
                  type="button"
                  onClick={() => selectAgent(agent.key)}
                  className={`w-full text-left rounded-lg px-2.5 py-2 transition-colors ${agent.key === selected ? 'bg-white/[0.08] text-zinc-100' : 'text-zinc-400 hover:bg-white/[0.04] hover:text-zinc-200'}`}
                >
                  <span className="block text-[12px] font-medium">{agent.display}</span>
                  <span className="block text-[10px] text-zinc-500 truncate">
                    {agent.effective.provider} · {agent.effective.model}
                  </span>
                </button>
              ))}
            </div>
          </aside>

          <div className="flex-1 min-w-0 overflow-y-auto" style={{ padding: '18px 22px' }}>
            {!entry ? (
              <p className="text-[12px] text-zinc-500">No agent selected.</p>
            ) : (
              <div className="space-y-4">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <h3 className="text-[13.5px] font-semibold text-zinc-100">{entry.display}</h3>
                    <p className="text-[10.5px] text-zinc-500">
                      tier: {entry.tier} · {entry.effective.provider_label} · {entry.effective.base_url}
                    </p>
                  </div>
                  <span
                    className="text-[10px] font-semibold rounded-full px-2 py-0.5 border"
                    style={{
                      color: entry.effective.has_key ? '#6ee7b7' : '#fbbf24',
                      borderColor: entry.effective.has_key ? 'rgba(110,231,183,0.25)' : 'rgba(251,191,36,0.3)',
                      background: entry.effective.has_key ? 'rgba(110,231,183,0.06)' : 'rgba(251,191,36,0.06)',
                    }}
                  >
                    {entry.effective.has_key ? 'key ready' : 'key missing'}
                  </span>
                </div>

                <div className="rounded-lg border border-white/[0.07] bg-white/[0.02] px-3 py-2 text-[10.5px] text-zinc-400 font-mono">
                  resolved: {entry.effective.provider} / {entry.effective.model} / {entry.effective.thinking || 'provider default'}
                </div>

                <div className="grid grid-cols-2 gap-3">
                  <div>
                    <label className={labelClass}>Provider</label>
                    <select
                      className={inputClass}
                      value={form.provider}
                      onChange={(event) => setForm((current) => ({ ...current, provider: event.target.value, model: '' }))}
                    >
                      <option value="">Inherit active ({activeProvider})</option>
                      {(payload?.providers || []).map((candidate) => (
                        <option key={candidate.id} value={candidate.id}>{candidate.label}</option>
                      ))}
                    </select>
                  </div>
                  <div>
                    <label className={labelClass}>Model</label>
                    <select
                      className={inputClass}
                      value={form.model}
                      onChange={(event) => setForm((current) => ({ ...current, model: event.target.value }))}
                    >
                      <option value="">Inherit tier default</option>
                      {models.map((model) => (
                        <option key={model.key} value={model.key}>{model.label} ({model.id})</option>
                      ))}
                    </select>
                  </div>
                  <div>
                    <label className={labelClass}>Reasoning effort</label>
                    <select
                      className={inputClass}
                      value={form.thinking}
                      onChange={(event) => setForm((current) => ({ ...current, thinking: event.target.value }))}
                    >
                      <option value="">Inherit tier default</option>
                      <option value="none">Off</option>
                      <option value="minimal">Minimal</option>
                      <option value="low">Low</option>
                      <option value="medium">Medium</option>
                      <option value="high">High</option>
                    </select>
                  </div>
                  <div>
                    <label className={labelClass}>MCP servers</label>
                    <input
                      className={inputClass}
                      value={form.mcp}
                      onChange={(event) => setForm((current) => ({ ...current, mcp: event.target.value }))}
                      placeholder="blank = all, none = none"
                    />
                  </div>
                </div>

                <div>
                  <label className={labelClass}>Tools (comma-separated)</label>
                  <input
                    className={inputClass}
                    value={form.tools}
                    onChange={(event) => setForm((current) => ({ ...current, tools: event.target.value }))}
                    placeholder="blank = scope default, none = no tools"
                  />
                </div>

                <div>
                  <label className={labelClass}>Skills</label>
                  <input
                    className={inputClass}
                    value={form.skills}
                    onChange={(event) => setForm((current) => ({ ...current, skills: event.target.value }))}
                    placeholder="blank = all skills, none = none"
                  />
                </div>

                <div>
                  <label className={labelClass}>System prompt (replaces the agent preamble)</label>
                  <textarea
                    className={`${inputClass} min-h-[92px] resize-y font-mono`}
                    value={form.systemPrompt}
                    onChange={(event) => setForm((current) => ({ ...current, systemPrompt: event.target.value }))}
                    placeholder="blank = built-in prompt"
                  />
                </div>

                {!!payload?.issues?.length && (
                  <div className="rounded-lg border border-amber-400/20 bg-amber-400/[0.04] px-3 py-2 text-[10.5px] text-amber-200/90 space-y-1">
                    {payload.issues.map((issue) => <div key={issue}>{issue}</div>)}
                  </div>
                )}

                {!!payload?.health?.[payload.active_provider] && (
                  <div className="rounded-lg border border-red-400/20 bg-red-400/[0.04] px-3 py-2 text-[10.5px] text-red-200/90">
                    Provider check failed: {payload.health[payload.active_provider]}
                  </div>
                )}

                {!!payload?.legacy_env?.length && (
                  <div className="rounded-lg border border-white/10 bg-white/[0.02] px-3 py-2 text-[10.5px] text-zinc-400">
                    Ignored legacy env: <span className="font-mono text-zinc-300">{payload.legacy_env.join(', ')}</span>.
                    Provider routing lives in <span className="font-mono">.deep_agents/config/providers.json</span>; these variables are not read.
                  </div>
                )}

                {error && <div className="text-[11px] text-red-300">{error}</div>}
                {status && <div className="text-[11px] text-emerald-300/90">{status}</div>}

                <div className="flex items-center justify-end gap-2 pt-1">
                  <button
                    type="button"
                    onClick={() => void reset()}
                    disabled={busy}
                    className="px-3 py-1.5 rounded-lg border border-white/10 text-[11.5px] text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.04] disabled:opacity-50"
                  >
                    Reset to built-in
                  </button>
                  <button
                    type="button"
                    onClick={() => void save()}
                    disabled={busy}
                    className="px-3.5 py-1.5 rounded-lg bg-indigo-500/90 text-[11.5px] font-semibold text-white hover:bg-indigo-400 disabled:opacity-50"
                  >
                    {busy ? 'Saving…' : 'Save agent'}
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
