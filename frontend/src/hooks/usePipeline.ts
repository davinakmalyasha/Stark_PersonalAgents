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
  run: (prompt: string, model: string, temp: number, workspacePath?: string, chatId?: string, attachments?: ChatAttachment[]) => void;
  cancel: () => void;
  state: AppState | null;
  isRunning: boolean;
  error: string | null;
  streaming: LiveStream;
}

const EMPTY_STREAM: LiveStream = { text: '', reasoning: '', agent: '', active: false, rate: 0 };

export function usePipeline(): UsePipelineReturn {
  const [state, setState] = useState<AppState | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [streaming, setStreaming] = useState<LiveStream>(EMPTY_STREAM);
  const abortRef = useRef<AbortController | null>(null);

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

  const run = useCallback((prompt: string, model: string, temp: number, workspacePath = '', chatId = '', attachments: ChatAttachment[] = []) => {
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
    );
  }, [applyDelta, resetStream]);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    resetStream();
    setIsRunning(false);
  }, [resetStream]);

  return { run, cancel, state, isRunning, error, streaming };
}
