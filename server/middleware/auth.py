"""
Remote access pairing token — Phase 7 moved from Desktop/remote_auth.py.
A random token persisted under .deep_agents/pairing.json; loopback always trusted.
"""
import json
import os
import secrets
import threading

_lock = threading.Lock()
_cache: dict = {"token": ""}


def _pairing_path() -> str:
    root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    return os.path.join(root, ".deep_agents", "pairing.json")


def get_pairing_token() -> str:
    """Return the persistent pairing token, generating it on first use."""
    with _lock:
        if _cache["token"]:
            return _cache["token"]
        path = _pairing_path()
        token = ""
        try:
            with open(path, "r", encoding="utf-8") as f:
                token = str(json.load(f).get("token", "")).strip()
        except Exception:
            token = ""
        if not token:
            token = secrets.token_hex(6)
            try:
                os.makedirs(os.path.dirname(path), exist_ok=True)
                with open(path, "w", encoding="utf-8") as f:
                    json.dump({"token": token}, f)
            except Exception as e:
                print(f"[AUTH] Could not persist pairing token ({e})")
        _cache["token"] = token
        return token


def reset_pairing_token() -> str:
    """Invalidate the current token and issue a fresh one."""
    with _lock:
        _cache["token"] = secrets.token_hex(6)
        try:
            path = _pairing_path()
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as f:
                json.dump({"token": _cache["token"]}, f)
        except Exception as e:
            print(f"[AUTH] Could not persist new pairing token ({e})")
        return _cache["token"]


def is_loopback(request) -> bool:
    """True when the request originates from this machine."""
    try:
        host = request.client.host if request.client else ""
    except Exception:
        host = ""
    return host in ("127.0.0.1", "::1", "localhost")
