"""Posts agent events to ForeMan's per-meeting callback. The agent never sends
email or contacts anyone else; only ForeMan does, under its policy engine."""

from __future__ import annotations

import json
import urllib.request


def post_event(url: str, token: str, event: dict, timeout: float = 15.0) -> dict:
    if not url.startswith(("https://", "http://")):
        raise ValueError("callback URL must be http(s)")
    allowed = {"consent": {"event", "granted", "statement", "at"}, "completed": {"event", "transcript", "summary", "scores"}}
    keys = allowed.get(event.get("event"))
    if keys is None:
        raise ValueError("unknown event")
    body = json.dumps({k: v for k, v in event.items() if k in keys}).encode()
    req = urllib.request.Request(url, data=body, method="POST", headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 (scheme checked above)
        return json.loads(resp.read() or b"{}")


INTERVIEW_KEYS = ("DEEPGRAM_API_KEY", "CARTESIA_API_KEY", "ANTHROPIC_API_KEY", "AXOM_AI_MODEL")


def fetch_keys(url: str, token: str, timeout: float = 15.0) -> dict:
    """The company's own AI keys and Claude model for this meeting (set in the ForeMan
    dashboard). Keys the company has not set are missing, and the agent uses
    its own from the environment. Any error means: use the agent's own keys."""
    if not url.startswith(("https://", "http://")):
        return {}
    req = urllib.request.Request(url, method="GET", headers={"Authorization": f"Bearer {token}"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310 (scheme checked above)
            keys = json.loads(resp.read() or b"{}").get("keys") or {}
    except Exception:  # noqa: BLE001 (fall back to the agent's own keys)
        return {}
    return {k: v for k, v in keys.items() if k in INTERVIEW_KEYS and isinstance(v, str) and v}
