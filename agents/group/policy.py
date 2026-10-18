"""agents.group.policy — what a project's envelope permits (Phase 5).

``docs/group-projects/04-autonomy-and-approvals.md`` §3 and §9.

**Pure.** No store, no clock, no I/O, no import of anything that has them. That
is deliberate and it is the same reason :func:`agents.group.planner.classify_shape`
is pure: the decision is the thing worth testing, and a decision tangled up
with a filesystem is a decision nobody tests.

``classify`` answers one question — *may this action proceed, wait for you, or
not happen at all* — and nothing else decides it. The envelope arrives as data
(``04`` §5, bullet 4): nothing under ``tools/`` imports this module or knows
that group projects exist.
"""
from __future__ import annotations

from dataclasses import dataclass

# ── tiers (``04`` §2) ───────────────────────────────────────────────────────

#: Do it; log it.
AUTONOMOUS = "autonomous"
#: Queue it, keep working — the item parks and a human is asked.
NEEDS_APPROVAL = "needs_approval"
#: Refuse and tell the human. The attempt is visible.
BLOCKED = "blocked"
#: Not offered at all. Handled by the materialised allowlist (``01`` §13),
#: not by this module — a tool that is absent has no action to classify.
NEVER = "never"

TIERS = (AUTONOMOUS, NEEDS_APPROVAL, BLOCKED, NEVER)

#: AU-01 — anything the envelope does not name waits for a person.
#: An enumerated allowlist is a list of things someone thought of; default-deny
#: catches the ones they did not (``04`` §3).
DEFAULT_TIER = NEEDS_APPROVAL


@dataclass(frozen=True)
class Action:
    """One thing a member wants to do.

    ``outside`` is about the project folder, not about the machine: a write to
    another part of the same repo is still outside. ``dangerous`` is AU-03's
    hard line — credentials, money, and ``_cmdpolicy``'s destructive set — and
    it outranks everything, including the envelope itself.
    """

    kind: str                      # "fs" | "command" | "mcp"
    target: str                    # a resolved path, a command root, "server/tool"
    write: bool = True
    outside: bool = False
    dangerous: bool = False


def shape_of(action: Action) -> str:
    """The identity a grant is remembered by (AU-08).

    Deliberately coarse: approving a write to a folder approves writes to that
    folder, not to the whole disk and not just one file. A pattern of approvals
    for the same shape is the signal that the envelope is wrong (§4.1) — so the
    shape has to be stable enough to show a pattern and narrow enough that
    approving once is not approving everything.
    """
    return f"{action.kind}:{'write' if action.write else 'read'}:{action.target}"


def _tier(value: object, default: str) -> str:
    """Read one envelope value.

    Absent means ``default``. Present but unrecognised — a typo, or a value
    from a future schema — is ``needs_approval``, never the permissive default.
    A misspelled envelope must not be a grant.
    """
    if value is None or str(value).strip() == "":
        return default
    text = str(value).strip().lower()
    return text if text in TIERS else NEEDS_APPROVAL


#: Path fragments that mark a credential store. This is AU-03's floor: outside
#: the project folder these are `blocked` at every tier, so no envelope and no
#: past approval can reach them.
#:
#: Scoped to *outside* on purpose. A project's own `.env` is the project's own
#: file, declared by whoever created it, and blocking it would make normal work
#: impossible while protecting nobody.
CREDENTIAL_MARKERS = (
    ".ssh", ".aws", ".gnupg", ".netrc", ".pypirc", ".npmrc",
    "id_rsa", "id_ed25519", "id_ecdsa",
    ".docker/config.json", ".config/gh", "credentials.json",
)


def looks_like_credential(target: str) -> bool:
    """True for a path that holds keys or tokens rather than content.

    ``.env`` is matched as a whole filename rather than a fragment, because as
    a fragment it also matches ``.environment`` and would block ordinary files.
    """
    lowered = str(target or "").replace("\\", "/").lower()
    if any(marker in lowered for marker in CREDENTIAL_MARKERS):
        return True
    name = lowered.rsplit("/", 1)[-1]
    return name == ".env" or name.startswith(".env.")


def classify(action: Action, *, envelope: dict | None = None,
             grants: object = ()) -> str:
    """The tier for one action. See ``04`` §3.

    Order matters, and each step is a rule rather than an implementation
    detail:

    1. **Inside the project folder is the free zone.** The folder is the unit
       of ownership (``01`` §2), so work in it is the project's own business —
       including a file that would be a credential anywhere else.
    2. **Dangerous is always blocked outside it.** AU-03: credentials and money
       are not automated at any tier. Checked before the envelope and before
       the grants, so nothing reaches past it.
    3. **Outside follows the envelope, default-deny** (AU-01).
    4. **A shape you have already approved is not gated again.** AU-08 §4.1 —
       the first outward action of a shape is checked, because that is what
       catches "it emailed the wrong vendor" before it emails forty more. Only
       ``needs_approval`` is lifted: a ``blocked`` action stays blocked however
       many times it is asked about.
    """
    envelope = envelope or {}

    if not action.outside:
        return _tier(envelope.get("project_folder"), AUTONOMOUS)

    if action.dangerous:
        return BLOCKED

    key = "external_write" if action.write else "external_read"
    tier = _tier(envelope.get(key), DEFAULT_TIER)
    if tier == NEEDS_APPROVAL and shape_of(action) in set(grants):
        return AUTONOMOUS
    return tier


__all__ = [
    "AUTONOMOUS",
    "BLOCKED",
    "CREDENTIAL_MARKERS",
    "DEFAULT_TIER",
    "NEEDS_APPROVAL",
    "NEVER",
    "TIERS",
    "Action",
    "classify",
    "looks_like_credential",
    "shape_of",
]
