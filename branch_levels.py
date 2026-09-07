"""branch_levels.py — single source of truth for the 3-level branch map.

The gate installer (tools/AddOnTool/githubTools.py) resolves branch levels
through this module:

    dev level      -> Tier 1 (fast-checks)     e.g. development / develop / dev
    staging level  -> Tier 2 (staging)          e.g. staging / qa / uat / test
    prod level     -> Tier 3 (release-heavy)    e.g. main / master / trunk

Resolution precedence:
    .harness-ci.json  >  env vars  >  discovery (remote heads scan)  >  defaults

Prod is ALWAYS a set: declared ∪ {main, master} ∪ detected-default.
Stdlib only — safe to import from anywhere.
"""
from __future__ import annotations

import json
import os

HARNESS_CONFIG = ".harness-ci.json"
DEV_CANDIDATES = ("development", "develop", "dev")
STAGING_CANDIDATES = ("staging", "stage", "stg", "qa", "uat", "test")
HARD_PROD = ("main", "master")
DEFAULT_DEV = "development"
DEFAULT_STAGING_FOR_RENDER = "staging"  # used when seeding needs A name


def load_repo_config(ws_or_path: str) -> dict:
    """Parse .harness-ci.json ({} when absent/unreadable)."""
    try:
        p = os.path.join(ws_or_path, HARNESS_CONFIG) \
            if os.path.isdir(ws_or_path) else ws_or_path
        with open(p, encoding="utf-8") as f:
            data = json.load(f)
        br = data.get("branches") or {}
        raw_prod = br.get("prod")
        prods = raw_prod if isinstance(raw_prod, list) else ([raw_prod] if raw_prod else [])
        return {
            "dev": str(br.get("dev") or "").strip(),
            "staging": str(br.get("staging") or "").strip(),
            "prod": [str(x).strip() for x in prods if str(x).strip()],
        }
    except Exception:
        return {}


def env_overrides(environ=None) -> dict:
    """CI_DEV_BRANCH / CI_STAGING_BRANCH / CI_PROD_BRANCHES."""
    env = environ if environ is not None else os.environ
    return {
        "dev": str(env.get("CI_DEV_BRANCH") or "").strip(),
        "staging": str(env.get("CI_STAGING_BRANCH") or "").strip(),
        "prod": [p.strip() for p in (env.get("CI_PROD_BRANCHES") or "").split(",") if p.strip()],
    }


def discover(heads, default: str) -> tuple[str, str]:
    """Scan remote branch names against well-known candidates.
    Returns (dev, staging); either may be '' when nothing matches."""
    heads = heads or []
    dev = next((c for c in DEV_CANDIDATES if c in heads), "")
    staging = next((c for c in STAGING_CANDIDATES if c in heads), "")
    return dev, staging


def normalize(declared: dict | None, envl: dict | None,
              heads=None, default: str = "main") -> dict:
    """Merge all signal layers into the canonical levels dict.

    Prod is ALWAYS a superset of {main, master} ∪ declared prod. The detected
    default branch joins prod too — UNLESS it IS the resolved dev level (fresh
    gated repos switch their default to the dev level; counting it as prod
    would poison the prod set and trip the collision guard forever).
    """
    d = declared or {}
    e = envl or {}

    heads = heads or []
    prod = {p for p in ((d.get("prod") or []) + (e.get("prod") or [])) if p}
    prod |= set(HARD_PROD)

    explicit_dev = d.get("dev") or e.get("dev") or ""
    dev = explicit_dev or ""
    staging = d.get("staging") or e.get("staging") or ""
    source = "declared"

    if not dev or not staging:
        disc_dev, disc_staging = discover(heads, default)
        if not dev and disc_dev:
            dev, source = disc_dev, "discovered"
        if not staging and disc_staging:
            staging = disc_staging

    if not dev:
        dev, source = DEFAULT_DEV, "default"

    if default and default != dev:
        prod.add(default)

    levels = {
        "dev": dev,
        "staging": staging,           # may be "" — unknown means "not wired"
        "prod": sorted(prod),
        "source": source,
        "default_branch": default,
    }
    levels["collision"] = dev in levels["prod"]
    return levels


def render_staging_name(levels: dict) -> str:
    """Name to bake into workflow templates when seeding (never '')."""
    return levels.get("staging") or DEFAULT_STAGING_FOR_RENDER
