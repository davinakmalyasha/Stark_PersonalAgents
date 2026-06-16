"""
Stage 3: LLM SDET --- Adversarial Testing & Forensic Diagnosis.

The SDET is a SEPARATE agent with a DIFFERENT model (deepseek-v4-flash, high reasoning effort (BSA_SDET_EFFORT override))
from the generator. It acts as the Developer's worst critic --- zero-trust philosophy,
assumes every line is broken until proven otherwise through rigorous analysis.

Uses invoke_messages_with_fallback for: model fallback chain, rate-limit retry,
concurrency throttling, automatic token tracking, and robust JSON parsing.
"""

import os
import json
from typing import Optional, Literal
from pydantic import BaseModel, Field


# ===============================================================================
# Pydantic output schema --- enforced via structured output / json_object mode
# ===============================================================================

class SdetIssue(BaseModel):
    """A single diagnosed defect --- gold-standard bug report format.

    Follows the industry-standard structure: what, where, how to reproduce,
    expected vs actual, evidence, and a non-prescriptive suggested direction.
    """
    # ── Location ──
    file: str = Field(description="Relative file path with the defect")
    line: int = Field(description="Line number (0 if not line-specific)")
    function: str = Field(
        default="",
        description="Function/method/component name where the defect lives",
    )

    # ── Classification ──
    severity: Literal["critical", "high", "medium", "low"] = Field(
        description="critical=blocks all functionality, high=breaks expected "
                    "behavior, medium=quality gap, low=style/minor"
    )
    category: str = Field(
        description="Category: validation, ux, accessibility, logic_error, "
                    "error_handling, test_failure, missing_feature, style, dependency"
    )

    # ── The Defect ──
    title: str = Field(
        default="",
        description="One-line summary following the formula: "
                    "'[Component] fails [Condition] causing [Impact]'. "
                    "Example: '[Login] form submits with empty fields --- no validation'"
    )
    steps_to_reproduce: str = Field(
        default="",
        description="Numbered, atomic steps with exact test data. Start from a "
                    "known state. Include the point where behavior deviates. "
                    "Example: '1. Navigate to /login\\n2. Enter email test@x.com\\n"
                    "3. Leave password empty\\n4. Click Sign In\\n5. Observe: no error'"
    )
    expected: str = Field(
        default="",
        description="What should happen per requirements/spec/reasonable expectation"
    )
    actual: str = Field(
        default="",
        description="What actually happened. Include exact error messages VERBATIM "
                    "from console/logs. Do NOT paraphrase errors."
    )
    evidence: str = Field(
        default="",
        description="Concrete proof: console errors (exact text), screenshot paths, "
                    "network response codes, stack traces, reproduction rate"
    )

    # ── Guidance (non-prescriptive) ──
    suggested_direction: str = Field(
        default="",
        description="Optional, non-prescriptive hint about the likely cause area. "
                    "The Developer decides how to fix. Describe the nature of the "
                    "problem, not the exact code change. Use 'Consider...' language."
    )
    confidence: float = Field(
        default=0.85,
        ge=0.0, le=1.0,
        description="Confidence in this diagnosis (0.0-1.0). "
                    "0.95+ = certain, 0.7-0.95 = likely, <0.7 = speculative"
    )

    # ── Backward-compatible aliases (for callers using old field names) ──
    @property
    def issue(self) -> str:
        """Backward-compat: returns title if set, otherwise composes from expected/actual."""
        if self.title:
            return self.title
        if self.expected and self.actual:
            return f"Expected: {self.expected}\nActual: {self.actual}"
        return ""

    @property
    def fix(self) -> str:
        """Backward-compat: returns suggested_direction."""
        return self.suggested_direction


class SdetDiagnosis(BaseModel):
    """Complete structured diagnosis from the SDET agent."""
    status: Literal["FIX_REQUIRED", "MINOR_ISSUES", "READY"] = Field(
        description="FIX_REQUIRED = blocker issues must be resolved before merge. "
                    "MINOR_ISSUES = can ship with caveats. "
                    "READY = all clear, no issues found."
    )
    summary: str = Field(
        description="1-3 sentence verdict: what went wrong (or why it's fine), "
                    "the root cause, and overall assessment."
    )
    issues: list[SdetIssue] = Field(
        default_factory=list,
        description="List of diagnosed issues, ordered by severity then confidence."
    )
    root_cause: str = Field(
        default="",
        description="If multiple issues share a common underlying cause, describe it."
    )
    files_to_read: list[str] = Field(
        default_factory=list,
        description="Files the Developer should read before applying fixes."
    )


# ===============================================================================
# SDET System Prompt --- loaded from sdet_prompt.py
# ===============================================================================
from sdet_prompt import _STATIC_SYSTEM_TEMPLATE as SDET_SYSTEM_PROMPT


# ===============================================================================
# Prompt builder
# ===============================================================================

def build_sdet_prompt(
    tracked_files: list[str],
    test_output: str,
    stage2_summary: str = "",
    stage2_findings: str = "",
) -> str:
    """
    Build the SDET's user prompt: failure signals + source code context.

    The prompt is structured so the model can cross-reference test failures
    against the exact source lines. File content is head-and-tail truncated
    to fit large files while preserving the most relevant sections.
    """
    parts = []

    parts.append(
        "## TASK\n"
        "You are the adversarial code reviewer. The Developer just wrote or "
        "modified these files. Tests may have failed. Your job:\n"
        "1. Diagnose every test failure --- trace each one to its root cause.\n"
        "2. Find bugs the tests missed --- security holes, logic errors, missing "
        "edge cases, performance traps.\n"
        "3. Produce a structured JSON diagnosis with exact file:line fixes.\n"
        "Assume the code is broken until you prove otherwise."
    )

    # ── Section 1: Failure Signals (ground truth) ──
    parts.append("\n## FAILURE SIGNALS (GROUND TRUTH)\n")

    if test_output:
        cleaned = test_output[:3000]
        if len(test_output) > 3000:
            cleaned += f"\n\n... (truncated --- {len(test_output)} total chars)"
        parts.append(f"### Test Output\n```\n{cleaned}\n```")
    else:
        parts.append(
            "### Test Output\n"
            "[No test output was provided. Analyze the source files for issues "
            "that should be caught: missing validation, error handling gaps, "
            "security vulnerabilities, untested edge cases.]"
        )

    if stage2_summary:
        parts.append(f"\n### Deterministic Check Summary\n{stage2_summary}")

    if stage2_findings:
        parts.append(f"\n### Deterministic Check Findings\n{stage2_findings}")

    # ── Section 2: Source Code ──
    parts.append("\n## SOURCE CODE (FILES WRITTEN OR MODIFIED)\n")

    for fpath in tracked_files:
        if not os.path.isfile(fpath):
            parts.append(f"\n### {fpath}\n[MISSING --- file was not created on disk]")
            continue

        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                code = f.read()

            if len(code) > 2500:
                # Head + tail: preserves imports/top-level structure AND
                # the bottom of the file where key logic often lives
                code = (
                    code[:1250]
                    + f"\n\n... ({len(code) - 2500} chars truncated) ...\n\n"
                    + code[-1250:]
                )

            ext = os.path.splitext(fpath)[1]
            parts.append(f"\n### {fpath}\n```{ext.lstrip('.') or 'text'}\n{code}\n```")

        except Exception as e:
            parts.append(f"\n### {fpath}\n[ERROR reading file: {e}]")

    # ── Section 3: Final Instructions ──
    parts.append(
        "\n## INSTRUCTIONS\n"
        "1. Start with the failure signals --- what exactly failed and why?\n"
        "2. Cross-reference each failure with the source files --- pinpoint the "
        "exact file and line responsible.\n"
        "3. Identify the ROOT CAUSE. One root cause (e.g., missing null check) "
        "can explain many symptoms (multiple test failures, crashes, edge cases).\n"
        "4. Audit for: security vulnerabilities, missing error handling, N+1 "
        "queries, race conditions, invalid HTML nesting, missing cleanup.\n"
        "5. Check test quality: are the tests real or mock-heavy placebos? "
        "Do they cover error paths or only happy paths?\n"
        "6. Produce your JSON diagnosis. Every issue must be specific and "
        "actionable --- the Developer should fix it without asking questions.\n"
        "7. Be honest about confidence. If you're not sure from the artifacts "
        "provided, flag it as speculative and suggest verification steps."
    )

    return "\n".join(parts)


# ===============================================================================
# Invoke the SDET
# ===============================================================================

def invoke_sdet(
    project_path: str = "",
    tracked_files: list[str] = None,
    test_output: str = "",
    stage2_summary: str = "",
    stage2_findings: str = "",
) -> SdetDiagnosis:
    """
    Invoke Stage 3 LLM SDET using deepseek-v4-flash with high reasoning effort (BSA_SDET_EFFORT override).

    Uses invoke_messages_with_fallback which provides:
    - Model fallback chain (v4-flash -> pro fallback -> others)
    - Rate-limit detection + 3 retries with exponential backoff
    - Concurrency throttling
    - Automatic token usage tracking
    - Robust JSON extraction + Pydantic validation

    Args:
        project_path: (deprecated, kept for backward compatibility) Project root.
        tracked_files: List of file paths the Developer wrote or modified.
        test_output: Captured stdout/stderr from running the tests.
        stage2_summary: Summary from the deterministic check stage.
        stage2_findings: Detailed findings from the deterministic check stage.

    Returns:
        SdetDiagnosis with status, summary, and actionable issues.
    """
    if tracked_files is None:
        tracked_files = []

    from llm import invoke_messages_with_fallback, trim_prompt, log_terminal
    from langchain_core.messages import SystemMessage, HumanMessage

    # Build the user prompt from artifacts
    prompt = build_sdet_prompt(
        tracked_files=tracked_files,
        test_output=test_output,
        stage2_summary=stage2_summary,
        stage2_findings=stage2_findings,
    )

    messages = [
        SystemMessage(content=trim_prompt(SDET_SYSTEM_PROMPT)),
        HumanMessage(content=trim_prompt(prompt)),
    ]

    where_tag = f"SDET: diagnose {len(tracked_files)} files"

    try:
        diagnosis = invoke_messages_with_fallback(
            role="SDET",
            messages=messages,
            temp=0.1,
            where=where_tag,
            schema=SdetDiagnosis,
            reasoning_effort=os.environ.get("BSA_SDET_EFFORT", "high"),
            model="deepseek-v4-flash",
        )

        # --- Auto-save diagnosis report to disk ---
        # Derive context from the tracked files for the report filename
        if tracked_files:
            import os as _os
            context = _os.path.splitext(_os.path.basename(tracked_files[0]))[0]
        else:
            context = "unknown"
        try:
            report_path = save_diagnosis_report(diagnosis, project_path or ".", context)
            log_terminal(f"[SDET] Report saved: {report_path}")
        except Exception as save_err:
            report_path = ""
            log_terminal(f"[SDET] Failed to save report: {save_err}")

        # Attach report_path for the caller to use
        diagnosis._report_path = report_path
        return diagnosis

    except Exception as e:
        # All models in the fallback chain failed.
        # Return a synthetic diagnosis so the pipeline doesn't crash.
        log_terminal(f"[SDET] All models exhausted: {e}")

        return SdetDiagnosis(
            status="FIX_REQUIRED",
            summary=(
                f"SDET invocation failed --- all models in the fallback chain "
                f"returned errors. Last error: {e}"
            ),
            issues=[SdetIssue(
                file="unknown",
                line=0,
                severity="high",
                category="dependency",
                issue=f"SDET LLM call failed: {str(e)[:300]}",
                fix=(
                    "Check DEEPSEEK_API_KEY validity, account balance at "
                    "https://platform.deepseek.com, and network connectivity. "
                    "Manual code review required for this change set."
                ),
                confidence=0.3,
            )],
            root_cause="SDET LLM invocation infrastructure failure.",
        )


# ===============================================================================
# Format diagnosis for Developer agent
# ===============================================================================

# ===============================================================================
# Diagnosis Report --- Save to disk for audit trail + Developer handoff
# ===============================================================================

def save_diagnosis_report(
    diagnosis: SdetDiagnosis,
    project_path: str = ".",
    context: str = "feature",
) -> str:
    """
    Persist the SDET diagnosis as a version-controlled markdown bug report.

    The Developer reads this file to understand what to fix. The report is
    designed to be committed alongside the code so every bug fix has a
    traceable diagnosis.

    Args:
        diagnosis: The SdetDiagnosis to save.
        project_path: Project root (report goes in {project_path}/reports/).
        context: Short slug describing the feature under test (e.g. 'login',
                 'checkout', 'calculator'). Used in the filename.

    Returns:
        Absolute path to the saved report file.
    """
    import os as _os
    from datetime import datetime as _dt, timezone as _tz

    reports_dir = _os.path.join(project_path, "reports")
    _os.makedirs(reports_dir, exist_ok=True)

    ts = _dt.now(_tz.utc).strftime("%Y%m%d-%H%M%S")
    safe_context = "".join(c if c.isalnum() or c in "-_" else "-" for c in context)[:40]
    filename = f"SDET-{safe_context}-{ts}.md"
    filepath = _os.path.join(reports_dir, filename)

    # --- Build the markdown report ---
    lines = [
        f"# SDET Diagnosis: {context}",
        "",
        f"**Date:** {_dt.now(_tz.utc).strftime('%Y-%m-%d %H:%M UTC')}",
        f"**Status:** `{diagnosis.status}`",
        f"**Issues Found:** {len(diagnosis.issues)}",
        "",
        "---",
        "",
        "## Summary",
        "",
        diagnosis.summary,
        "",
    ]

    if diagnosis.root_cause:
        lines.extend(["## Root Cause", "", diagnosis.root_cause, ""])

    if diagnosis.files_to_read:
        lines.append("## Files to Read Before Fixing")
        for f in diagnosis.files_to_read:
            lines.append(f"- `{f}`")
        lines.append("")

    if diagnosis.issues:
        lines.append("## Issues")
        lines.append("")
        # Count by severity
        sev_counts: dict[str, int] = {}
        for iss in diagnosis.issues:
            sev_counts[iss.severity] = sev_counts.get(iss.severity, 0) + 1
        lines.append(f"| Severity | Count |")
        lines.append(f"|----------|-------|")
        for sev in ["critical", "high", "medium", "low"]:
            if sev in sev_counts:
                lines.append(f"| **{sev.upper()}** | {sev_counts[sev]} |")
        lines.append("")

        for i, issue in enumerate(diagnosis.issues, start=1):
            sev_icon = {"critical": "🔴", "high": "🟠", "medium": "🟡", "low": "⚪"}
            icon = sev_icon.get(issue.severity, "⚪")
            title_text = issue.title or f"{issue.file}:{issue.line}"

            lines.append(f"### {i}. {icon} [{issue.severity.upper()}] {title_text}")
            lines.append("")

            lines.append(f"| Field | Detail |")
            lines.append(f"|-------|--------|")
            lines.append(f"| **Location** | `{issue.file}:{issue.line}`"
                         + (f" -> `{issue.function}()`" if issue.function else "") + " |")
            lines.append(f"| **Category** | `{issue.category}` |")
            lines.append(f"| **Confidence** | {issue.confidence:.0%} |")
            lines.append("")

            if issue.steps_to_reproduce:
                lines.append("**Steps to Reproduce:**")
                for step in issue.steps_to_reproduce.strip().split("\n"):
                    if step.strip():
                        lines.append(f"  {step.strip()}")
                lines.append("")

            if issue.expected:
                lines.append(f"**✅ Expected:** {issue.expected}")
                lines.append("")
            if issue.actual:
                lines.append(f"**❌ Actual:** {issue.actual}")
                lines.append("")

            if issue.evidence:
                lines.append(f"**📎 Evidence:** {issue.evidence}")
                lines.append("")

            if issue.suggested_direction:
                lines.append(f"**💡 Suggested Direction:** {issue.suggested_direction}")
                lines.append("")

    lines.extend([
        "---",
        "",
        f"*Generated by SDET Agent (deepseek-v4-flash + high reasoning effort (BSA_SDET_EFFORT override))*",
    ])

    with open(filepath, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))

    return _os.path.abspath(filepath)


def format_diagnosis_for_developer(
    diagnosis: SdetDiagnosis,
    report_path: str = "",
) -> str:
    """
    Convert a SdetDiagnosis into a human-readable message the Developer agent
    can execute directly.

    Issues are ordered by severity (critical -> high -> medium -> low), and the
    Developer is told to apply fixes in that exact order.

    Args:
        diagnosis: The SDET diagnosis to format.
        report_path: Optional path to the saved .md report file. If provided,
                     the Developer is told to read that file for full details.
    """
    lines = [
        "[STAGE 3 --- SDET DIAGNOSIS]",
        "",
        f"**Status:** {diagnosis.status}",
        f"**Summary:** {diagnosis.summary}",
    ]

    if report_path:
        lines.append(f"**Full Report:** {report_path} (read this file for complete details)")

    if diagnosis.root_cause:
        lines.append(f"**Root Cause:** {diagnosis.root_cause}")

    if diagnosis.files_to_read:
        lines.append(f"**Files to read before fixing:** {', '.join(diagnosis.files_to_read)}")

    if diagnosis.issues:
        lines.append("")
        lines.append("## Issues to Fix (apply in order)")
        lines.append("")

        for i, issue in enumerate(diagnosis.issues, start=1):
            title_text = issue.title or f"{issue.file}:{issue.line or '?'}"
            func_suffix = f" -> `{issue.function}()`" if issue.function else ""
            lines.append(
                f"### Issue {i}: [{issue.severity.upper()}] {title_text}"
            )
            lines.append(f"- **Location:** `{issue.file}:{issue.line}`{func_suffix}")
            lines.append(f"- **Category:** {issue.category} | **Confidence:** {issue.confidence:.0%}")
            if issue.steps_to_reproduce:
                lines.append(f"- **Steps to Reproduce:**")
                for step in issue.steps_to_reproduce.strip().split("\n"):
                    if step.strip():
                        lines.append(f"  {step.strip()}")
            if issue.expected:
                lines.append(f"- **Expected:** {issue.expected}")
            if issue.actual:
                lines.append(f"- **Actual:** {issue.actual}")
            if issue.evidence:
                lines.append(f"- **Evidence:** {issue.evidence}")
            if issue.suggested_direction:
                lines.append(f"- **Suggested Direction:** {issue.suggested_direction}")
            lines.append("")

    # Action footer based on status
    if diagnosis.status == "FIX_REQUIRED":
        lines.append(
            "## ACTION REQUIRED\n\n"
            "Review the issues above and address them in severity order "
            "(critical -> high -> medium). Each issue includes steps to reproduce, "
            "expected vs actual behavior, and evidence. The `suggested_direction` "
            "field points toward the likely cause --- use your judgment on the "
            "best implementation.\n\n"
            "After addressing critical and high issues, re-run the tests "
            "to verify the fixes."
        )
    elif diagnosis.status == "MINOR_ISSUES":
        lines.append(
            "## ACTION (Optional)\n\n"
            "Issues are minor quality gaps --- the feature works but has room "
            "for improvement. Address them if you have remaining cycles; "
            "otherwise document and ship."
        )
    elif diagnosis.status == "READY":
        lines.append(
            "## ALL CLEAR\n\n"
            "No defects found. The feature passes all tests and quality checks. "
            "Ready to merge."
        )

    return "\n".join(lines)


# ===============================================================================
# Browser Mode --- Multi-Turn Agent Loop for Live Black-Box Testing
# ===============================================================================

def invoke_sdet_browser(
    app_url: str,
    test_focus: str = "",
    max_turns: int = 20,
) -> SdetDiagnosis:
    """
    Invoke the SDET in LIVE browser-testing mode with full tool access.

    This runs a multi-turn agent loop: the SDET navigates to the running app,
    interacts with every element, probes for security holes, checks console
    errors, tests keyboard navigation, and produces a structured SdetDiagnosis.

    Uses the same deepseek-v4-flash + high reasoning effort (BSA_SDET_EFFORT override) as the static mode, but with
    the full browser tool suite available for real black-box testing.

    Args:
        app_url: Full URL of the running application (e.g. http://localhost:3000).
        test_focus: Optional specific feature/flow to test. If empty, tests everything.
        max_turns: Maximum tool-calling turns before forcing a diagnosis.

    Returns:
        SdetDiagnosis with bugs found through live browser testing.
    """
    import json as _json
    import re as _re
    from llm import invoke_messages_with_fallback, trim_prompt, log_terminal
    from langchain_core.messages import (
        SystemMessage, HumanMessage, AIMessage, ToolMessage,
    )
    from tools import execute_tool, TOOL_DEFINITIONS

    # --- Filter to SDET-relevant tools (only tools that exist in TOOL_MAP) ---
    sdet_tool_names = {
        "browser_navigate", "browser_click", "browser_fill", "browser_type",
        "browser_press", "browser_screenshot", "browser_extract", "browser_close",
        "read_file", "search_code", "list_files", "run_command",
    }
    sdet_tools = [t for t in TOOL_DEFINITIONS if t["name"] in sdet_tool_names]

    # --- Build the mission prompt ---
    focus_block = (
        f"**Specific focus area:** {test_focus}\n"
        if test_focus else
        "Perform comprehensive black-box testing of ALL features and user flows."
    )

    user_prompt = f"""## LIVE BROWSER TESTING MISSION

**App URL:** {app_url}
{focus_block}

You have FULL browser automation tools. Your job is to BREAK this application
and find every bug, security hole, and UX issue before the Developer ships it.

### Mandatory Testing Protocol

**Phase 1 --- Reconnaissance:**
1. Navigate to {app_url}
2. Take a screenshot: `browser_screenshot()`
3. Read the page structure: `browser_extract()`
4. Map every interactive element to its @ref tag

**Phase 2 --- Happy Path:**
- Walk through every documented user flow step by step
- Verify each page/screen renders correctly
- Verify success messages, redirects, and data persistence

**Phase 3 --- Chaos & Fuzzing (for EVERY form and input):**
- Submit the form completely empty -> verify validation errors appear
- Fill every text field with 5000 'A' characters -> must not crash
- Fill every text field with SQL injection: `' OR '1'='1` -> must not execute
- Fill every text field with XSS: `<script>alert(1)</script>` -> must be sanitized
- Fill numeric fields with: 0, -1, 99999999999, 1.00001
- Double-click every submit button rapidly -> button must disable after first click
- Submit, press browser Back, submit again -> must handle gracefully

**Phase 4 --- Console & Network Audit (after EVERY interaction):**
- Extract the full page text with `browser_extract(what="text")` and scan it for JS console errors, React hydration warnings, or missing-`key` warnings surfaced in the DOM
- Any red error = CRITICAL bug. Any React hydration warning = CRITICAL bug.
- Any missing `key` prop warning = HIGH bug.

**Phase 5 --- Keyboard & Accessibility:**
- Navigate using ONLY Tab and Shift+Tab through every interactive element
- Verify visible focus ring on every focused element (screenshot with `browser_screenshot()`)
- Open any modal -> verify focus is trapped inside -> press Escape -> verify focus returns
- Verify touch targets are at least 24x24px by screenshotting and inspecting the rendered size

**Phase 6 --- Session & Security:**
- If auth exists: log in as User A, try to access User B's resources (IDOR)
- Log out, verify token is invalidated (can't access authenticated pages)
- Check that sensitive tokens are NOT in localStorage

**Phase 7 --- Responsive:**
- Test at viewport widths: 375px, 768px, 1024px
- Verify no horizontal scrollbar at mobile widths

### Rules
- ALWAYS use @ref selectors from browser_navigate / browser_extract snapshots (e.g. '@e4'), NOT CSS classes
- On Windows, wrap @ref selectors in quotes: `browser_click('@e1')`
- Wait for page loads between navigations
- Take screenshots of every bug as evidence
- When you find a bug, keep testing --- don't stop at the first issue
- After completing ALL testing phases, produce your final SdetDiagnosis JSON

### Final Output
When you have thoroughly tested the application, produce a single JSON object
matching the SdetDiagnosis schema with all issues found, ordered by severity.

**Start immediately. Navigate to {app_url} NOW.**"""

    messages = [
        SystemMessage(content=trim_prompt(SDET_SYSTEM_PROMPT)),
        HumanMessage(content=trim_prompt(user_prompt)),
    ]

    tool_outputs_summary: list[str] = []

    for turn in range(max_turns):
        log_terminal(f"[SDET Browser] Turn {turn + 1}/{max_turns}")

        try:
            result = invoke_messages_with_fallback(
                role="SDET",
                messages=messages,
                temp=0.1,
                where=f"SDET browser: turn {turn + 1}/{max_turns}",
                tools=sdet_tools,
                reasoning_effort=os.environ.get("BSA_SDET_EFFORT", "high"),
                model="deepseek-v4-flash",
            )
        except Exception as e:
            log_terminal(f"[SDET Browser] LLM call failed on turn {turn + 1}: {e}")
            return SdetDiagnosis(
                status="FIX_REQUIRED",
                summary=f"SDET browser testing failed on turn {turn + 1}: {e}",
                issues=[SdetIssue(
                    file="unknown", line=0, severity="high",
                    category="dependency",
                    issue=f"LLM invocation failed during browser testing: {str(e)[:300]}",
                    fix="Check API key, network, and model availability.",
                    confidence=0.3,
                )],
                root_cause="SDET LLM infrastructure error in browser mode.",
            )

        text, tool_calls, reasoning = result

        # --- No tool calls = model is done testing, parse final diagnosis ---
        if not tool_calls:
            log_terminal(f"[SDET Browser] Testing complete after {turn + 1} turns --- parsing diagnosis")
            raw = text or ""

            # Extract JSON from response (model may wrap in markdown or add commentary)
            json_match = _re.search(r'\{.*\}', raw, _re.DOTALL)
            if json_match:
                raw = json_match.group(0)

            try:
                data = _json.loads(raw)
                diagnosis = SdetDiagnosis.model_validate(data)
            except Exception as e:
                diagnosis = SdetDiagnosis(
                    status="FIX_REQUIRED",
                    summary=f"SDET browser testing completed but JSON parsing failed: {e}",
                    issues=[SdetIssue(
                        file="unknown", line=0, severity="high",
                        category="test_failure",
                        issue=f"SDET produced unparseable output after {turn + 1} browser testing turns.",
                        fix="Review the raw SDET output manually:\n" + raw[:500],
                        confidence=0.4,
                    )],
                    root_cause="SDET final output was not valid JSON.",
                )

            # --- Auto-save report ---
            context = test_focus[:40] if test_focus else "browser-test"
            try:
                report_path = save_diagnosis_report(diagnosis, ".", context)
                log_terminal(f"[SDET Browser] Report saved: {report_path}")
            except Exception:
                report_path = ""
            diagnosis._report_path = report_path
            return diagnosis

        # --- Execute tool calls and append results to conversation ---
        for tc in tool_calls:
            tool_name = tc.get("tool", tc.get("name", ""))
            tool_args = tc.get("args", {})

            # Brief log line
            arg_preview = _json.dumps(tool_args)
            if len(arg_preview) > 120:
                arg_preview = arg_preview[:117] + "..."
            log_terminal(f"[SDET Browser]  -> {tool_name}({arg_preview})")

            # Execute the tool
            try:
                tool_result = execute_tool(tool_name, tool_args)
                tool_result_str = str(tool_result)
                # Truncate very long outputs to stay within context limits
                if len(tool_result_str) > 4000:
                    tool_result_str = (
                        tool_result_str[:2000]
                        + "\n\n... (truncated middle) ...\n\n"
                        + tool_result_str[-2000:]
                    )
            except Exception as e:
                tool_result_str = f"Tool execution error: {e}"

            tool_outputs_summary.append(f"{tool_name}: {tool_result_str[:200]}")

            # Build call ID for message correlation
            call_id = f"call_{turn}_{tool_name}_{len(tool_outputs_summary)}"

            # Append the assistant's tool-call message
            ai_msg = AIMessage(
                content="",
                tool_calls=[{
                    "id": call_id,
                    "name": tool_name,
                    "args": tool_args,
                }],
            )
            # Append the tool result message
            tool_msg = ToolMessage(
                content=tool_result_str,
                tool_call_id=call_id,
            )
            messages.append(ai_msg)
            messages.append(tool_msg)

    # --- Max turns exhausted without final diagnosis ---
    log_terminal(f"[SDET Browser] WARNING: max turns ({max_turns}) reached without final diagnosis")
    return SdetDiagnosis(
        status="FIX_REQUIRED",
        summary=f"SDET browser testing exceeded max turns ({max_turns}) without producing a final diagnosis.",
        issues=[SdetIssue(
            file="unknown", line=0, severity="medium",
            category="test_failure",
            issue=(
                f"SDET ran {max_turns} browser testing turns without completing. "
                f"Last actions: {'; '.join(tool_outputs_summary[-5:]) if tool_outputs_summary else 'none'}"
            ),
            fix="Re-run with a narrower test focus or review the tool outputs above.",
            confidence=0.3,
        )],
        root_cause="SDET browser agent exceeded turn limit without finalizing.",
    )
