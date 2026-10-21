"""Time triggers that wake the orchestrator."""
from __future__ import annotations

import threading
import time

from server.store import agent_wakes, schedules

_started = False
_lock = threading.Lock()
_stop = threading.Event()


def _tick() -> None:
    for item in schedules.due_schedules(limit=20):
        text = (
            f"[SCHEDULED WAKE] {item.get('name') or 'Scheduled wake'}\n"
            f"{item.get('prompt') or ''}"
        ).strip()
        try:
            agent_wakes.enqueue_wake(
                item.get("workspacePath") or "",
                item.get("chatId") or "",
                text,
                {
                    "scheduleId": item.get("id"),
                    "scheduleName": item.get("name"),
                    "scheduled": True,
                },
                mode="orchestrator",
            )
            schedules.mark_enqueued(item["id"])
        except Exception as exc:
            print(f"[SCHEDULE] enqueue failed for {item.get('id')}: {exc}", flush=True)


def _loop() -> None:
    while not _stop.is_set():
        try:
            _tick()
        except Exception as exc:
            print(f"[SCHEDULE] scheduler error: {exc}", flush=True)
        _stop.wait(5.0)


def start_schedule_scheduler() -> bool:
    global _started
    with _lock:
        if _started:
            return False
        _started = True
        _stop.clear()
    threading.Thread(target=_loop, name="schedule-scheduler", daemon=True).start()
    return True


def stop_schedule_scheduler() -> None:
    global _started
    _stop.set()
    with _lock:
        _started = False
    time.sleep(0.05)
