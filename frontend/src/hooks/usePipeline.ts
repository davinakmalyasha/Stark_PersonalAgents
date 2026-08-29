import { useState, useCallback, useRef } from 'react';
import type { AppState, ChatAttachment, StreamDelta } from '../types';
import { streamPipeline } from '../services/api';

export interface LiveStream {
  text: string;
  reasoning: string;
  agent: string;
  active: boolean;
  rate: number; // tokens/sec estimate
}

interface UsePipelineReturn {
  run: (prompt: string, model: string, temp: number, workspacePath?: string, chatId?: string, attachments?: ChatAttachment[], workspaceKind?: string) => void;
  cancel: () => void;
  state: AppState | null;
  isRunning: boolean;
  error: string | null;
  streaming: LiveStream;
  /** How the current/last run reached this client: initiated here or mirrored from another client. */
  runMode: 'local' | 'mirror' | null;
  /** Watch an existing live run started by another client (phone ↔ laptop mirroring). */
  attachMirror: (chatId: string) => void;
}

const EMPTY_STREAM: LiveStream = { text: '', reasoning: '', agent: '', active: false, rate: 0 };

export function usePipeline(): UsePipelineReturn {
  const [state, setState] = useState<AppState | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [streaming, setStreaming] = useState<LiveStream>(EMPTY_STREAM);
  const abortRef = useRef<AbortController | null>(null);
  const [runMode, setRunMode] = useState<'local' | 'mirror' | null>(null);

  // tok/s estimation: chars since last delta / seconds since last delta, smoothed
  const rateRef = useRef({ lastTs: 0, lastChars: 0, ema: 0 });
  const streamRef = useRef({ text: '', reasoning: '', agent: '' });

  const applyDelta = useCallback((delta: StreamDelta) => {
    const now = performance.now();
    if (delta.kind === 'reasoning') {
      streamRef.current.reasoning += delta.text;
    } else if (delta.kind === 'text') {
      const chars = delta.text.length;
      const r = rateRef.current;
      if (r.lastTs > 0) {
        const dt = Math.max(16, now - r.lastTs) / 1000;
        const rate = (chars / 4) / dt; // ~4 chars per token
        r.ema = r.ema === 0 ? rate : r.ema * 0.8 + rate * 0.2;
      }
      r.lastTs = now;
      r.lastChars = chars;
      streamRef.current.text += delta.text;
    } else if (delta.kind === 'end') {
      rateRef.current.ema = 0;
    }
    if (delta.agent) streamRef.current.agent = delta.agent;
    setStreaming({
      text: streamRef.current.text,
      reasoning: streamRef.current.reasoning,
      agent: streamRef.current.agent,
      active: true,
      rate: Math.round(rateRef.current.ema),
    });
  }, []);

  const resetStream = useCallback(() => {
    streamRef.current = { text: '', reasoning: '', agent: '' };
    rateRef.current = { lastTs: 0, lastChars: 0, ema: 0 };
    setStreaming(EMPTY_STREAM);
  }, []);

  const run = useCallback((prompt: string, model: string, temp: number, workspacePath = '', chatId = '', attachments: ChatAttachment[] = [], workspaceKind = '') => {
    setRunMode('local');
    setIsRunning(true);
    setError(null);
    resetStream();
    setState({
      active_node: 'supervisor',
      next_agent: '',
      completed_nodes: [],
      thoughts: { supervisor: 'Analyzing request...', developer: '' },
      client_request: prompt,
      outputs: {
        requirements: '',
        tech_spec: '',
        code: '',
        test_report: '',
        devops_config: '',
        analytics_report: '',
        gherkin: '',
        mermaid: '',
      },
      project_path: workspacePath,
      agents_plan: '',
      antigravity_log: [],
      live_terminal_log: `[THOUGHT] [Supervisor] Analyzing request...\n`,
      selected_model: model,
      selected_temp: temp,
      token_usage: {
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cost: 0,
        calls: [],
      },
    });
    abortRef.current?.abort();
    abortRef.current = streamPipeline(
      prompt, model, temp,
      (newState) => setState(newState),
      () => { resetStream(); setIsRunning(false); },
      (err) => { setError(err); resetStream(); setIsRunning(false); },
      workspacePath,
      chatId,
      attachments,
      applyDelta,
      workspaceKind,
    );
  }, [applyDelta, resetStream]);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    resetStream();
    setIsRunning(false);
    setRunMode(null);
  }, [resetStream]);

  /**
   * Live mirroring (mobile/remote feature): subscribe to /api/stream/{chatId}
   * and feed another client's run frames into this client's pipeline state.
   * Errors are NOT raised locally — the initiating client owns persistence;
   * mirrors only render and refresh on completion.
   */
  const attachMirror = useCallback((chatId: string) => {
    if (!chatId) return;
    setRunMode('mirror');
    setIsRunning(true);
    setError(null);
    resetStream();

    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    (async () => {
      try {
        const res = await fetch(`/api/stream/${encodeURIComponent(chatId)}`, { signal: controller.signal });
        if (!res.ok || !res.body) {
          setRunMode(null);
          setIsRunning(false);
          return;
        }
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        let currentEvent = '';
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split('\n');
          buffer = lines.pop() ?? '';
          for (const line of lines) {
            if (line.startsWith('event: ')) {
              currentEvent = line.slice(7).trim();
              continue;
            }
            if (!line.startsWith('data: ')) continue;
            try {
              const parsed = JSON.parse(line.slice(6).trim());
              if (currentEvent === 'delta') {
                applyDelta(parsed as StreamDelta);
              } else if (currentEvent === 'done') {
                resetStream();
                setIsRunning(false);
                return;
              } else if (parsed.error) {
                // Render the failure state but leave persistence to the initiator.
                setState(parsed as AppState);
                resetStream();
                setIsRunning(false);
                return;
              } else {
                setState(parsed as AppState);
              }
            } catch { /* partial line */ }
            currentEvent = '';
          }
        }
        // Stream closed without an explicit done — stop mirroring quietly.
        resetStream();
        setIsRunning(false);
      } catch (err) {
        if ((err as Error)?.name !== 'AbortError') {
          resetStream();
          setIsRunning(false);
        }
      }
    })();
  }, [applyDelta, resetStream]);

  return { run, cancel, state, isRunning, error, streaming, runMode, attachMirror };
}
