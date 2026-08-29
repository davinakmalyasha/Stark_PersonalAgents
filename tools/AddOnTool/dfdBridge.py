"""DFD site bridge — HTTP client to the DFD Agency API (non-money operations).

The DFD agent talks to the DFD website through this tool. It is deliberately
scoped to agency OPERATIONS (leads, orders, milestones, notes, activities,
client requests, projects, articles, whatsapp) and NEVER touches money
records (payments, invoices, contracts, pricing, promo, billing).

Configuration (env, loaded from .env if present):
- DFD_API_URL   — base URL of the DFD Express server, e.g. https://api.dfdagency.com
- DFD_API_KEY   — secret key for the /api/agent/* endpoints (site-side agentBridge)
- DFD_API_TIMEOUT — request timeout seconds (default 30)
"""

from __future__ import annotations

import json
import os
import urllib.request
import urllib.error

def _cfg() -> dict:
    try:
        from dotenv import load_dotenv

        load_dotenv()
    except Exception:
        pass
    return {
        "url": (os.environ.get("DFD_API_URL") or "").strip().rstrip("/"),
        "key": (os.environ.get("DFD_API_KEY") or "").strip(),
        "timeout": float(os.environ.get("DFD_API_TIMEOUT") or "30"),
    }


#: Bridge endpoints the DFD site exposes for the agent (non-money only).
#: Key -> (method, path template). Path placeholders like {id} are filled
#: from the `path` params of the request.
BRIDGE_ENDPOINTS = {
    # Leads
    "lead_list":        ("GET", "/api/agent/leads"),
    "lead_get":         ("GET", "/api/agent/leads/{id}"),
    "lead_update":      ("PATCH", "/api/agent/leads/{id}"),
    # Orders (read + progress only — no money fields)
    "order_list":       ("GET", "/api/agent/orders"),
    "order_get":        ("GET", "/api/agent/orders/{id}"),
    "order_milestones": ("GET", "/api/agent/orders/{id}/milestones"),
    # Milestones (create/update/view)
    "milestone_create": ("POST", "/api/agent/orders/{orderId}/milestones"),
    "milestone_update": ("PATCH", "/api/agent/milestones/{id}"),
    # Order notes (create/update)
    "note_create":      ("POST", "/api/agent/orders/{orderId}/notes"),
    "note_update":      ("PATCH", "/api/agent/notes/{id}"),
    # Activities (append)
    "activity_create":  ("POST", "/api/agent/orders/{orderId}/activities"),
    # Client requests (read/respond)
    "request_list":     ("GET", "/api/agent/requests"),
    "request_get":      ("GET", "/api/agent/requests/{id}"),
    "request_update":   ("PATCH", "/api/agent/requests/{id}"),
    # Projects (read/update — non-money fields only)
    "project_list":     ("GET", "/api/agent/projects"),
    "project_get":      ("GET", "/api/agent/projects/{id}"),
    "project_update":   ("PATCH", "/api/agent/projects/{id}"),
    # Articles (content)
    "article_list":     ("GET", "/api/agent/articles"),
    "article_create":   ("POST", "/api/agent/articles"),
    "article_update":   ("PATCH", "/api/agent/articles/{id}"),
    # WhatsApp (send progress notification to a client number)
    "whatsapp_send":    ("POST", "/api/agent/whatsapp/send"),
}


def _request(method: str, url: str, key: str, payload: dict | None, timeout: float) -> str:
    """Low-level HTTP call; returns the response body or an 'Error: ...' string."""
    try:
        body = None
        headers = {
            "Authorization": f"Bearer {key}",
            "Accept": "application/json",
        }
        if payload is not None:
            headers["Content-Type"] = "application/json"
            body = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(url, data=body, headers=headers, method=method.upper())
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            try:
                data = json.loads(raw)
                return json.dumps(data, ensure_ascii=False)[:4000]
            except Exception:
                return raw[:4000]
    except urllib.error.HTTPError as e:
        detail = ""
        try:
            detail = e.read().decode("utf-8", errors="replace")[:500]
        except Exception:
            pass
        return f"Error: DFD API {e.code} {e.reason}: {detail}"
    except urllib.error.URLError as e:
        return f"Error: DFD API unreachable: {e.reason}"
    except Exception as e:
        return f"Error: DFD API request failed: {e}"


def dfd_bridge(
    action: str = "",
    id: str = "",
    orderId: str = "",
    payload: dict | None = None,
    path: dict | None = None,
    method: str = "GET",
    url: str = "",
    timeout: float | None = None,
) -> str:
    """Call the DFD Agency API through the agent bridge (non-money only).

    Use a named endpoint:

        dfd_bridge(action="lead_list")
        dfd_bridge(action="lead_get", id="5")
        dfd_bridge(action="lead_update", id="5", payload={"status": "CONTACTED"})
        dfd_bridge(action="order_get", id="<order-uuid>")
        dfd_bridge(action="milestone_create", orderId="<uuid>",
                   payload={"title": "...", "description": "...", "category": "FRONTEND"})
        dfd_bridge(action="note_create", orderId="<uuid>", payload={"content": "..."})
        dfd_bridge(action="activity_create", orderId="<uuid>",
                   payload={"type": "MILESTONE", "message": "..."})
        dfd_bridge(action="whatsapp_send", payload={"to": "+62...", "message": "..."})

    Advanced usage (raw): pass ``method`` + ``url`` (relative to DFD_API_URL,
    e.g. "/api/agent/orders") and optional ``payload``. ``path`` optionally
    fills placeholders when ``url`` contains ``{key}`` tokens.

    Never passes money fields. If you need payment/contract/invoice data,
    ask the owner manually — this tool is scoped to operations only.
    """
    cfg = _cfg()
    base = cfg["url"]
    key = cfg["key"]
    to = timeout or cfg["timeout"]

    if not base:
        return "Error: DFD_API_URL is not set. Configure it in the environment or .env."

    if not url:
        ep = BRIDGE_ENDPOINTS.get(action or "")
        if not ep:
            known = ", ".join(sorted(BRIDGE_ENDPOINTS))
            return f"Error: unknown dfd_bridge action '{action}'. Known: {known}"
        method, url = ep

    # Fill path placeholders {key} from `path` or top-level id/orderId params.
    params = dict(path or {})
    if "{id}" in url:
        params.setdefault("id", id)
    if "{orderId}" in url:
        params.setdefault("orderId", orderId)
    for k, v in params.items():
        url = url.replace("{" + k + "}", str(v))

    full = base + (url if url.startswith("/") else "/" + url)
    if not key:
        return f"Error: DFD_API_KEY is not set; cannot call {full}."
    return _request(method, full, key, payload, to)
