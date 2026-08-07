import { useState, useCallback, useRef } from 'react';
import type { AppState, ChatAttachment } from '../types';
import { streamPipeline } from '../services/api';

interface UsePipelineReturn {
  run: (prompt: string, model: string, temp: number, workspacePath?: string, chatId?: string, attachments?: ChatAttachment[]) => void;
  cancel: () => void;
  state: AppState | null;
  isRunning: boolean;
  error: string | null;
}

export function usePipeline(): UsePipelineReturn {
  const [state, setState] = useState<AppState | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const run = useCallback((prompt: string, model: string, temp: number, workspacePath = '', chatId = '', attachments: ChatAttachment[] = []) => {
    setIsRunning(true);
    setError(null);
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
      () => setIsRunning(false),
      (err) => { setError(err); setIsRunning(false); },
      workspacePath,
      chatId,
      attachments,
    );
  }, []);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    setIsRunning(false);
  }, []);

  return { run, cancel, state, isRunning, error };
}
