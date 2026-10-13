"""agents.group.criteria — acceptance criteria evaluation (Phase 2).

``docs/group-projects/01-concept-model.md`` section 4.6.

An item's acceptance criteria **are** its definition of done, which is why the
project's completion predicate is built out of them rather than judged
separately (``00`` section 6.3). Evaluation is deterministic wherever it can
be, and explicitly *pending* where it cannot.

| kind | Evaluated by | Cost |
|---|---|---|
| ``check`` | A named harness check — file, glob, regex, size, command | free |
| ``manual`` | A human, via the inbox. The item stays ``doing`` | one approval |
| ``llm`` | A bounded single-call judge — **Phase 3** | one call |
| ``budget`` | Always-on, enforced by the supervisor's gate | free |

Every path in a criterion is **relative to the project root** and is rejected
if it escapes it (P-1, and the ``never`` tier for paths outside the project).
"""
from __future__ import annotations

import glob
import os
import re
import subprocess
from dataclasses import dataclass, field
from typing import Any

#: A criterion that runs a shell command is a capability. Bounded hard.
COMMAND_TIMEOUT = 120
COMMAND_OUTPUT_CAP = 4000

PASS = "pass"
FAIL = "fail"
PENDING = "pending"
UNSUPPORTED = "unsupported"


class CriterionPathError(Exception):
    """A criterion tried to reach outside the project root."""


@dataclass
class Result:
    criterion: dict
    status: str
    detail: str = ""
    output: str = ""

    @property
    def ok(self) -> bool:
        return self.status == PASS


@dataclass
class Evaluation:
    results: list[Result] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return bool(self.results) and all(r.ok for r in self.results)

    @property
    def pending(self) -> list[Result]:
        return [r for r in self.results if r.status == PENDING]

    @property
    def failed(self) -> list[Result]:
        return [r for r in self.results if r.status == FAIL]

    @property
    def unsupported(self) -> list[Result]:
        return [r for r in self.results if r.status == UNSUPPORTED]

    def summary(self) -> str:
        if not self.results:
            return "no criteria"
        if self.passed:
            return "all criteria passed"
        if self.pending:
            return "awaiting: " + "; ".join(r.detail for r in self.pending)
        if self.failed:
            return "failed: " + "; ".join(r.detail for r in self.failed)
        return "unverifiable: " + "; ".join(r.detail for r in self.unsupported)


def _safe_path(root: str, relative: str) -> str:
    """Resolve ``relative`` under ``root``, refusing to escape it."""
    if not relative:
        raise CriterionPathError("criterion has no path")
    if os.path.isabs(relative) or re.match(r"^[A-Za-z]:[\\/]", relative):
        # P-1: project files never carry absolute paths.
        raise CriterionPathError(f"absolute path not allowed: {relative!r}")
    root_abs = os.path.abspath(root)
    target = os.path.abspath(os.path.join(root_abs, relative))
    if target != root_abs and not target.startswith(root_abs + os.sep):
        raise CriterionPathError(f"path escapes the project root: {relative!r}")
    return target


# ── individual checks ───────────────────────────────────────────────────────


def _check_file_exists(criterion: dict, root: str) -> Result:
    path = _safe_path(root, str(criterion.get("path") or ""))
    if os.path.isfile(path):
        return Result(criterion, PASS, f"file exists: {criterion.get('path')}")
    return Result(criterion, FAIL, f"missing file: {criterion.get('path')}")


def _check_dir_exists(criterion: dict, root: str) -> Result:
    path = _safe_path(root, str(criterion.get("path") or ""))
    if os.path.isdir(path):
        return Result(criterion, PASS, f"dir exists: {criterion.get('path')}")
    return Result(criterion, FAIL, f"missing dir: {criterion.get('path')}")


def _check_min_bytes(criterion: dict, root: str) -> Result:
    path = _safe_path(root, str(criterion.get("path") or ""))
    want = int(criterion.get("bytes") or 1)
    if not os.path.isfile(path):
        return Result(criterion, FAIL, f"missing file: {criterion.get('path')}")
    size = os.path.getsize(path)
    if size >= want:
        return Result(criterion, PASS, f"{criterion.get('path')} is {size} bytes")
    return Result(criterion, FAIL,
                  f"{criterion.get('path')} is {size} bytes, need {want}")


def _check_glob_nonempty(criterion: dict, root: str) -> Result:
    pattern = str(criterion.get("pattern") or "")
    if not pattern or os.path.isabs(pattern):
        return Result(criterion, FAIL, "glob pattern must be relative")
    matches = glob.glob(os.path.join(root, pattern), recursive=True)
    if matches:
        return Result(criterion, PASS, f"{len(matches)} match(es) for {pattern}")
    return Result(criterion, FAIL, f"no files match {pattern}")


def _check_regex(criterion: dict, root: str) -> Result:
    path = _safe_path(root, str(criterion.get("path") or ""))
    pattern = str(criterion.get("pattern") or "")
    if not pattern:
        return Result(criterion, FAIL, "regex criterion has no pattern")
    if not os.path.isfile(path):
        return Result(criterion, FAIL, f"missing file: {criterion.get('path')}")
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            text = handle.read()
    except OSError as exc:
        return Result(criterion, FAIL, f"unreadable: {exc}")
    if re.search(pattern, text, re.MULTILINE):
        return Result(criterion, PASS, f"{criterion.get('path')} matches /{pattern}/")
    return Result(criterion, FAIL, f"{criterion.get('path')} does not match /{pattern}/")


def _check_command(criterion: dict, root: str) -> Result:
    """Run a bounded command inside the project root.

    Note this is the ONE place the supervisor executes something. It makes no
    LLM calls (RT-01), but it does spawn a process — so the command comes from
    the human-authored manifest, runs with ``cwd`` pinned to the project root,
    and is capped by a timeout.
    """
    command = criterion.get("command")
    if not command:
        return Result(criterion, FAIL, "command criterion has no command")
    cwd = root
    if criterion.get("cwd"):
        cwd = _safe_path(root, str(criterion["cwd"]))
    if isinstance(command, str):
        argv = command
        shell = True
    else:
        argv = [str(a) for a in command]
        shell = False
    try:
        proc = subprocess.run(
            argv, cwd=cwd, shell=shell, capture_output=True, text=True,
            timeout=float(criterion.get("timeout") or COMMAND_TIMEOUT),
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            check=False,
        )
    except subprocess.TimeoutExpired:
        return Result(criterion, FAIL, f"command timed out: {command}")
    except OSError as exc:
        return Result(criterion, FAIL, f"command failed to start: {exc}")
    expect = int(criterion.get("expect_exit", 0))
    output = ((proc.stdout or "") + (proc.stderr or ""))[:COMMAND_OUTPUT_CAP]
    if proc.returncode == expect:
        return Result(criterion, PASS, f"exit {proc.returncode}", output=output)
    return Result(criterion, FAIL,
                  f"exit {proc.returncode}, expected {expect}", output=output)


_CHECKS = {
    "file_exists": _check_file_exists,
    "dir_exists": _check_dir_exists,
    "min_bytes": _check_min_bytes,
    "glob_nonempty": _check_glob_nonempty,
    "regex": _check_regex,
    "command": _check_command,
}


# ── entry points ────────────────────────────────────────────────────────────


def evaluate_criterion(criterion: dict, root: str,
                       judge: Any = None) -> Result:
    """Evaluate one criterion. Never raises — a bad criterion is a FAIL.

    ``judge`` is an optional callable ``(criterion, root) -> (bool, str)`` for
    the ``llm`` kind. It is injected rather than imported so that the
    supervisor's tick stays free of any model dependency (RT-01): the app
    wires a real judge, tests wire a fake, and with no judge at all an ``llm``
    criterion is reported ``unsupported`` rather than guessed.
    """
    if not isinstance(criterion, dict):
        return Result({"kind": "?"}, FAIL, f"malformed criterion: {criterion!r}")
    kind = str(criterion.get("kind") or "check").lower()

    if kind == "manual":
        return Result(criterion, PENDING,
                      str(criterion.get("prompt") or "awaiting human review"))
    if kind == "llm":
        if judge is None:
            return Result(criterion, UNSUPPORTED,
                          "llm criterion with no judge configured")
        try:
            verdict = judge(criterion, root)
        except Exception as exc:  # noqa: BLE001 - a broken judge is not a pass
            return Result(criterion, FAIL, f"judge error: {type(exc).__name__}: {exc}")
        if isinstance(verdict, tuple):
            passed, detail = bool(verdict[0]), str(verdict[1] if len(verdict) > 1 else "")
        else:
            passed, detail = bool(verdict), ""
        return Result(criterion, PASS if passed else FAIL,
                      detail or ("judged acceptable" if passed else "judged unacceptable"))
    if kind == "budget":
        return Result(criterion, PASS, "budget is enforced by the gate")

    name = str(criterion.get("check") or "").lower()
    handler = _CHECKS.get(name)
    if handler is None:
        return Result(criterion, UNSUPPORTED, f"unknown check {name!r}")
    try:
        return handler(criterion, root)
    except CriterionPathError as exc:
        return Result(criterion, FAIL, str(exc))
    except Exception as exc:  # noqa: BLE001 - a broken check is a FAIL, not a crash
        return Result(criterion, FAIL, f"{type(exc).__name__}: {exc}")


def evaluate_item(item: dict, root: str, judge: Any = None) -> Evaluation:
    """Evaluate every criterion on an item. An item with none fails."""
    criteria = item.get("acceptance") or []
    if not criteria:
        return Evaluation([Result({}, FAIL, "item has no acceptance criteria")])
    return Evaluation([evaluate_criterion(c, root, judge) for c in criteria])


def describe(criterion: dict) -> str:
    """One-line human description, for the board and the inbox."""
    kind = str(criterion.get("kind") or "check").lower()
    if kind == "manual":
        return f"manual: {criterion.get('prompt') or 'review'}"
    if kind == "llm":
        return f"llm: {criterion.get('prompt') or 'judge'}"
    name = str(criterion.get("check") or "?")
    target = criterion.get("path") or criterion.get("pattern") or criterion.get("command") or ""
    return f"{name}: {target}".strip()
