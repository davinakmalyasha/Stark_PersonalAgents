// Log parser utility for Stark trace visualization

export interface TraceStep {
  id: string;
  type: 'thought' | 'tool' | 'command' | 'system';
  agent: string;
  title: string;
  content: string;
  toolName?: string;
  args?: string;
  isError?: boolean;
  duration?: string;
  /** Spawned child session id — set by [SUBAGENT] lines; enables drill-in. */
  childId?: string;
  /** Short display label for the spawned child (task-derived). */
  label?: string;
  /** The spawned agent type (UI, DevOps…) — distinct from step.agent (the caller). */
  childName?: string;
  /** Workspace id the child runs in (e.g. _general) — fresh delegations include it. */
  childWorkspace?: string;
}

export interface GroupedTraceStep {
  id: string;
  type: 'thought' | 'exploration' | 'edit' | 'command' | 'system';
  title: string;
  agent: string;
  steps: TraceStep[];
  isError?: boolean;
}

/** Max chars for tool output content — file contents can be huge; cap for the UI */
const MAX_TOOL_CONTENT = 4000;

export function normalizeAgentName(name: string): string {
  const n = name.trim().toLowerCase();
  if (n === 'developer' || n === 'dev') return 'Dev';
  if (n === 'analytics') return 'Analytics';
  if (n === 'designer' || n === 'ui/ux designer') return 'Designer';
  // 'supervisor' is the legacy label for Stark in old traces — never displayed.
  if (n === 'supervisor' || n === 'orchestrator' || n === 'stark') return 'Stark';
  return name;
}

/**
 * Tool names that spawn subagent child sessions. `task` is the legacy name,
 * `call_subagent` the async variant, `delegate` the orchestrator's parallel
 * fan-out tool. Every render/matching gate must use this helper — never
 * hard-code a subset — otherwise delegated children detach from their call
 * site and fall back to the top-hoisted card.
 */
const DELEGATION_TOOL_NAMES: ReadonlySet<string> = new Set(['task', 'call_subagent', 'delegate', 'spawn_agent', 'message_agent']);

export function isDelegationToolName(name: string | undefined): boolean {
  return !!name && DELEGATION_TOOL_NAMES.has(name);
}

/**
 * Extracts the {name, task} delegation target from tool args. Supports both
 * shapes: task/call_subagent ({name, task}) and delegate
 * ({items: [{agent, task, ...}]} — one item per parallel child).
 */
export function delegationTarget(args?: string): { name: string; task: string } {
  if (!args) return { name: '', task: '' };
  try {
    const parsed = JSON.parse(args) as Record<string, unknown>;
    if (typeof parsed.task_name === 'string') {
      return { name: normalizeAgentName(String(parsed.task_name)), task: String(parsed.message || '') };
    }
    if (typeof parsed.name === 'string' || typeof parsed.task === 'string') {
      return { name: normalizeAgentName(String(parsed.name || '')), task: String(parsed.task || '') };
    }
    const items = parsed.items;
    const first = Array.isArray(items) ? (items[0] as Record<string, unknown> | undefined) : undefined;
    if (first && typeof first === 'object') {
      return { name: normalizeAgentName(String(first.agent || first.name || '')), task: String(first.task || '') };
    }
  } catch {
    // Best effort — args are display-only.
  }
  return { name: '', task: '' };
}

/**
 * Lines that should be silently skipped (not appended to any step's content).
 * These are log-formatting / progress artefacts — the real tool data is captured
 * by the "🔧 Calling" and "[TOOL OUTPUT]" lines.
 */
const SKIP_LINE_PATTERNS = [
  // Developer action indicators (redundant with 🔧 Calling lines)
  /^\[DEVELOPER\]\s+write\s/,          // "[DEVELOPER] write foo.ts (435B)"
  /^\[DEVELOPER\]\s+read\s/,           // "[DEVELOPER] read foo.ts"
  /^\[DEVELOPER\]\s+edit\s/,           // "[DEVELOPER] edit foo.ts"
  /^\[DEVELOPER\]\s+run\s/,            // "[DEVELOPER] run npm test"
  /^\[DEVELOPER\]\s+search\s/,         // "[DEVELOPER] search \"pattern\""
  /^\[DEVELOPER\]\s+ls\s/,             // "[DEVELOPER] ls ."
  /^\[DEVELOPER\]\s+delegate\s/,       // "[DEVELOPER] delegate Dev"
  /^\[DEVELOPER\]\s+async\s/,          // "[DEVELOPER] async Dev"
  /^\[DEVELOPER\]\s+=>\s/,             // "[DEVELOPER]   => foo.ts #1-10/20"
  /^\[DEVELOPER\]\s+Response\s*\(/,    // "[DEVELOPER] Response (94 chars)"
  /^\[DEVELOPER\]\s+âœ…\s+Agent finished/, // Agent finished status line
  // Tool output metadata (real content is in the [TOOL OUTPUT] line)
  /^\[FILE\]\s+/,                       // "[FILE] backend/src/foo.ts (lines 1-15 of 15)"
  /^\[OK\]\s+/,                         // "[OK] Wrote backend/src/foo.ts (15 lines, 435 chars)"
  // Iteration / progress markers
  /^\[DEV\s+Iteration\s+\d+\/\d+\]/,   // "[DEV Iteration 10/50]"
  /^\[Developer\]\s+Iter\s+\d+\/\d+:/, // "[Developer] Iter 10/50: write foo.ts"
  /^\[ORCH\s+Iteration\s+\d+\/\d+\]/, // "[ORCH Iteration 10/20]"
  // Separators & status
  /^={10,}/,                            // "==============" separator lines
];

function isSkippableLine(line: string): boolean {
  for (const pattern of SKIP_LINE_PATTERNS) {
    if (pattern.test(line)) return true;
  }
  return false;
}

function truncateContent(content: string, maxLen: number): string {
  if (content.length <= maxLen) return content;
  return content.substring(0, maxLen) + `\n... (truncated ${content.length - maxLen} chars)`;
}

/** Parses the full raw live_terminal_log into steps and final response */
export function parseLogToSteps(log: string): { steps: TraceStep[]; assistantResponse: string | null } {
  const lines = log.split('\n');
  const steps: TraceStep[] = [];
  let assistantResponse: string | null = null;
  let currentStep: TraceStep | null = null;
  let stepCounter = 0;
  let activeAgent = 'Stark';

  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    // Check for agent headers like "--- [BA AGENT] ---"
    const agentHeaderMatch = trimmed.match(/^---\s+\[(\w+)\s+AGENT\]\s+---$/i);
    if (agentHeaderMatch) {
      activeAgent = normalizeAgentName(agentHeaderMatch[1]);
      continue;
    }

    // Check for thoughts like "[DEVELOPER THOUGHTS] >> reasoning content"
    const thoughtMatch = trimmed.match(/^\[(\w+)\s+THOUGHTS\]\s+>>\s+(.*)$/i);
    if (thoughtMatch) {
      activeAgent = normalizeAgentName(thoughtMatch[1]);
      currentStep = {
        id: `step-${stepCounter++}`,
        type: 'thought',
        agent: activeAgent,
        title: `${activeAgent} Thoughts`,
        content: thoughtMatch[2],
      };
      steps.push(currentStep);
      continue;
    }

    if (trimmed.startsWith('[THOUGHT]')) {
      const match = trimmed.match(/^\[THOUGHT(?:[:\s]+(\d+s?))?\]\s+\[([^\]]+)\]\s+(.*)$/);
      if (match) {
        const duration = match[1];
        activeAgent = normalizeAgentName(match[2]);
        currentStep = {
          id: `step-${stepCounter++}`,
          type: 'thought',
          agent: activeAgent,
          title: `${activeAgent} Thought`,
          content: match[3],
          duration,
        };
        steps.push(currentStep);
      }
    } else if (trimmed.startsWith('[THINKING]')) {
      // [THINKING] Agent: reasoning — real-time developer status messages
      const match = trimmed.match(/^\[THINKING(?:[:\s]+(\d+s?))?\]\s+(\w+):\s+(.*)$/);
      if (match) {
        const duration = match[1];
        activeAgent = normalizeAgentName(match[2]);
        currentStep = {
          id: `step-${stepCounter++}`,
          type: 'thought',
          agent: activeAgent,
          title: `${activeAgent} Thinking`,
          content: match[3],
          duration,
        };
        steps.push(currentStep);
      }
    } else if (trimmed.startsWith('[TOOL_CALL]')) {
      // [TOOL_CALL] tool_name(args)
      const match = trimmed.match(/^\[TOOL_CALL\]\s+(\w+)\((.*)\)$/);
      if (match) {
        const tool = match[1];
        let args = match[2];
        let displayTitle = tool === 'run_command' ? 'Running command' : `Calling ${tool}`;

        // Try to parse as JSON for cleaner field extraction and specialized task titles
        try {
          const parsed = JSON.parse(args);
          if (isDelegationToolName(tool)) {
            if (tool === 'delegate') {
              // Parallel fan-out: one item per child ({agent, task, ...}).
              const items = Array.isArray(parsed.items) ? parsed.items : [];
              if (items.length === 1) {
                const subagentName = normalizeAgentName(String(items[0]?.agent || items[0]?.name || ''));
                if (subagentName) displayTitle = `Delegating to ${subagentName}`;
              } else if (items.length > 1) {
                displayTitle = `Delegating to ${items.length} agents`;
              }
            } else if (parsed.name) {
              parsed.name = normalizeAgentName(parsed.name);
              const subagentName = parsed.name;
              displayTitle = tool === 'task'
                ? `Delegating to ${subagentName}`
                : `Subagent ${subagentName}`;
            }
          }
          args = JSON.stringify(parsed);
        } catch {
          // Not valid JSON — keep raw args string but truncate if huge
          if (args.length > 500) args = args.substring(0, 500) + '...';
        }
        currentStep = {
          id: `step-${stepCounter++}`,
          type: tool === 'run_command' ? 'command' : 'tool',
          agent: activeAgent,
          title: displayTitle,
          toolName: tool,
          args,
          content: '',
        };
        steps.push(currentStep);
      }
    } else if (trimmed.startsWith('[DELEGATION STARTED]')) {
      // Legacy parallel delegate output is followed by rows containing the
      // durable child chat id. The row parser below attaches those inline.
    } else if (/^\s*-\s*del-[\w-]+\s*[·|]/.test(trimmed)) {
      const row = trimmed.match(/^\s*-\s*(del-[\w-]+)\s*[·|]\s*([^·|]+?)\s*[·|]\s*(.+?)\s*[—-].*?\(chat:\s*([^)]+)\)/i);
      if (row) {
        const childId = row[4].trim();
        const spawnName = normalizeAgentName(row[2].trim());
        const label = row[3].trim();
        if (currentStep && isDelegationToolName(currentStep.toolName) && !currentStep.childId) {
          currentStep.childId = childId;
          currentStep.label = label;
          currentStep.childName = spawnName;
        } else {
          currentStep = {
            id: `step-${stepCounter++}`,
            type: 'system',
            agent: spawnName,
            title: label || spawnName,
            content: '',
            childId,
            label,
            childName: spawnName,
          };
          steps.push(currentStep);
        }
      }
    } else if (trimmed.startsWith('[SPAWNED]')) {
      const spawned = trimmed.match(/^\[SPAWNED\]\s+child=([^\s;]+)(?:\s+name=([^;]+))?/i);
      if (spawned) {
        const childId = spawned[1].trim();
        const spawnName = normalizeAgentName((spawned[2] || 'Subagent').trim());
        const argsTask: string = currentStep?.args ? delegationTarget(currentStep.args).task : '';
        const label: string = argsTask || spawnName;
        if (currentStep && isDelegationToolName(currentStep.toolName) && !currentStep.childId) {
          currentStep.childId = childId;
          currentStep.label = label;
          currentStep.childName = spawnName;
        } else {
          currentStep = {
            id: `step-${stepCounter++}`,
            type: 'system',
            agent: spawnName,
            title: label,
            content: '',
            childId,
            label,
            childName: spawnName,
          };
          steps.push(currentStep);
        }
      }
    } else if (trimmed.startsWith('[SUBAGENT]')) {
      // [SUBAGENT] child=<id> name=<Agent> [ws=<workspace>] [chat=<chat_id>] label=<short task label>
      // New delegations include ws/chat (which chat Stark is talking in / checking);
      // old traces omit them — parser tolerates both shapes.
      let childId = '', spawnName = '', spawnLabel = '', spawnWs = '';
      const m2: RegExpMatchArray | null = trimmed.match(/^\[SUBAGENT\]\s+child=(\S+)\s+name=(\S+)\s+ws=(\S+)\s+chat=(\S+)\s+label=(.*?)(?:\s+del=\S+)?\s*$/);
      if (m2) {
        childId = m2[1]; spawnName = m2[2]; spawnWs = m2[3]; spawnLabel = (m2[5] || '').trim();
      } else {
        const m3 = trimmed.match(/^\[SUBAGENT\]\s+child=(\S+)\s+name=(\S+)\s+label=(.*?)(?:\s+del=\S+)?\s*$/);
        if (!m3) { /* unparseable SUBAGENT line */ } else { childId = m3[1]; spawnName = m3[2]; spawnLabel = (m3[3] || '').trim(); }
      }
      if (childId) {
        const label = spawnLabel.trim().replace(/\s+del=del-\S+$/, '');
        const isDelegationStep = !!currentStep && isDelegationToolName(currentStep.toolName);
        // Attach the child to its delegation tool step so it renders inline
        // at the call site. A parallel `delegate` fan-out emits one [SUBAGENT]
        // line per item — once the step already owns a child, extra children
        // become standalone steps (never overwrite) so no child is lost.
        if (isDelegationStep && currentStep && !currentStep.childId) {
          currentStep.childId = childId;
          currentStep.label = label || currentStep.title;
          currentStep.childName = normalizeAgentName(spawnName);
          if (spawnWs) currentStep.childWorkspace = spawnWs;
        } else {
          currentStep = {
            id: `step-${stepCounter++}`,
            type: 'system',
            agent: normalizeAgentName(spawnName),
            title: label || normalizeAgentName(spawnName),
            content: '',
            childId,
            label,
            childName: normalizeAgentName(spawnName),
            ...(spawnWs ? { childWorkspace: spawnWs } : {}),
          };
          steps.push(currentStep);
        }
      }
    } else if (trimmed.startsWith('[TOOL]')) {
      // Skip TOOL header prefix line
    } else if (trimmed.startsWith('[TOOL OUTPUT]')) {
      const match = trimmed.match(/^\[TOOL OUTPUT\]\s+(\w+):\s+(.*)$/i);
      if (match) {
        const toolName = match[1];
        const resultPreview = match[2];
        if (currentStep && currentStep.toolName === toolName) {
          currentStep.content = truncateContent(resultPreview, MAX_TOOL_CONTENT);
        }
      }
    } else if (trimmed.startsWith('[ORCHESTRATOR] Summary:')) {
      assistantResponse = trimmed.slice(23).trim();
      currentStep = null;
    } else if (trimmed.startsWith('[DEVELOPER] Summary:')) {
      assistantResponse = trimmed.slice(20).trim();
      currentStep = null;
    } else if (
      trimmed.startsWith('[ORCH Iteration') ||
      trimmed.startsWith('[ORCHESTRATOR]') ||
      trimmed.startsWith('[Orchestrator]') ||
      (trimmed.startsWith('[THINKING] Supervisor:') || trimmed.startsWith('[THINKING] Stark:')) // legacy token tolerated
    ) {
      currentStep = null;
    } else if (trimmed.startsWith('[ASSISTANT]')) {
      assistantResponse = trimmed.slice(12).trim();
      currentStep = null;
    } else if (trimmed.startsWith('[USER]') || trimmed.startsWith('[SESSION]')) {
      currentStep = null;
    } else {
      // Catch-all: only append to assistantResponse, never to a step.
      // Previously this appended everything to currentStep.content, which
      // caused tool results / file contents / dev log lines to bleed into
      // thought bubbles and create massive unreadable DOM nodes.
      if (assistantResponse !== null && currentStep === null) {
        assistantResponse += '\n' + trimmed;
      } else if (currentStep && !isSkippableLine(trimmed)) {
        // Only append if it looks like actual content (not log metadata)
        currentStep.content += (currentStep.content ? '\n' : '') + trimmed;
        // Cap tool output content (file contents can be enormous).
        // Thought content is not capped — the UI has overflow scroll.
        if (currentStep.type !== 'thought' && currentStep.content.length > MAX_TOOL_CONTENT) {
          currentStep.content = truncateContent(currentStep.content, MAX_TOOL_CONTENT);
        }
        if (currentStep.content.includes('[ERR]') || currentStep.content.includes('Error:')) {
          currentStep.isError = true;
        }
      }
    }
  }

  return { steps, assistantResponse };
}

/** Groups consecutive exploration/edit tool steps for visual summary */
export function groupTraceSteps(steps: TraceStep[]): GroupedTraceStep[] {
  const grouped: GroupedTraceStep[] = [];
  let currentGroup: GroupedTraceStep | null = null;
  let groupCounter = 0;

  for (const step of steps) {
    if (step.type === 'tool') {
      // Delegation calls must never be grouped as exploration — treat as standalone
      const isDelegation = isDelegationToolName(step.toolName);
      if (isDelegation) {
        currentGroup = {
          id: step.id,
          type: 'system',
          title: step.title,
          agent: step.agent,
          steps: [step],
          isError: step.isError,
        };
        grouped.push(currentGroup);
        currentGroup = null; // break any open group
        continue;
      }

      const isEdit = step.toolName === 'write_file' || step.toolName === 'write' || step.toolName === 'edit_file' || step.toolName === 'edit';
      const expectedType = isEdit ? 'edit' : 'exploration';

      if (currentGroup && currentGroup.type === expectedType) {
        currentGroup.steps.push(step);
        if (step.isError) currentGroup.isError = true;

        // Update title with count
        const count = currentGroup.steps.length;
        currentGroup.title = isEdit
          ? `Edited ${count} file${count > 1 ? 's' : ''}`
          : `Explored ${count} file${count > 1 ? 's' : ''}`;
      } else {
        currentGroup = {
          id: `group-${groupCounter++}`,
          type: expectedType,
          title: isEdit ? 'Edited 1 file' : 'Explored 1 file',
          agent: step.agent,
          steps: [step],
          isError: step.isError,
        };
        grouped.push(currentGroup);
      }
    } else {
      currentGroup = {
        id: step.id,
        type: step.type === 'command' ? 'command' : step.type === 'thought' ? 'thought' : 'system',
        title: step.title,
        agent: step.agent,
        steps: [step],
        isError: step.isError,
      };
      grouped.push(currentGroup);
      currentGroup = null; // Break grouping
    }
  }

  return grouped;
}

/**
 * Orchestrator-only steps for the main chat.
 *
 * The main chat should only display orchestrator-level actions:
 * delegation (task / call_subagent / delegate), supervisor thoughts, and
 * final summaries. Subagent tool calls (read_file, write_file, run_command,
 * search_code, etc.) belong in the canvas subagent drawer, not the main chat.
 *
 * Steps from Stark (the orchestrator) + any task delegation
 * tool call are kept; everything else is stripped.
 */
export function filterOrchestratorSteps(steps: TraceStep[]): TraceStep[] {
  return steps.filter((s) => {
    // Always keep Stark / orchestrator thoughts ('Supervisor' is the legacy label)
    if (s.agent === 'Stark' || s.agent === 'Supervisor' || s.agent === 'Orchestrator') return true;

    // Keep delegation calls regardless of which agent issued them
    // ('task' is the legacy delegation tool name in old traces)
    if (isDelegationToolName(s.toolName)) return true;

    // Subagent internals — drop
    return false;
  });
}

/**
 * Subagent spawn steps from a persisted trace message's metadata.steps —
 * only steps carrying a childId (drill-in capable) come back.
 */
export function extractSubagentSteps(steps: unknown): TraceStep[] {
  if (!Array.isArray(steps)) return [];
  return steps.filter(
    (s): s is TraceStep => !!s && typeof s === 'object' && typeof (s as TraceStep).childId === 'string',
  );
}
