"""Regression: server.house.service must tolerate both on-disk schemas.

house_state.json exists as a list-schema (written by the workspace UI:
devices/scenes/rooms are lists of {id, ...}) and as the legacy dict-keyed
seed ({"devices": {id: {...}}}). The full crash was:

    'list' object has no attribute 'values'  (house_summary on list schema)

which killed every House run at prompt-build time (house_agent injects the
summary). These tests pin both shapes plus the tool-wrapper contract
(skipped entries, history series).
"""
import json
import os
import sys

import pytest

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)
DESKTOP = os.path.join(REPO_ROOT, "Desktop")
if DESKTOP not in sys.path:
    sys.path.insert(0, DESKTOP)

from server.house import service


LIST_STATE = {
    "rooms": [{"id": "living", "name": "Living Room", "icon": "sofa"}],
    "devices": [
        {"id": "light-1", "name": "Ceiling", "roomId": "living", "type": "light",
         "state": {"power": True, "brightness": 55}, "capabilities": ["power"]},
        {"id": "lock-1", "name": "Door", "roomId": "living", "type": "lock",
         "state": {"locked": False}, "capabilities": ["locked"]},
        {"id": "thermo-1", "name": "Thermo", "roomId": "living", "type": "thermostat",
         "state": {"targetTemp": 21.0, "mode": "auto"}, "capabilities": ["targetTemp"]},
    ],
    "scenes": [
        {"id": "scene-1", "name": "Evening",
         "actions": [{"deviceId": "light-1", "state": {"power": False}}]},
    ],
    "energy": {"today_kwh": 1.5},
    "security": {},
}

DICT_STATE = {
    "rooms": ["living"],
    "devices": {
        "living_light_1": {"name": "Lamp", "type": "light", "room": "living",
                           "state": {"on": True, "brightness": 70}},
        "front_lock": {"name": "Lock", "type": "lock", "room": "living",
                       "state": {"locked": True}},
    },
    "scenes": {
        "night": {"name": "Night",
                  "actions": {"living_light_1": {"on": False}}},
    },
    "energy": {"today_kwh": 2.0},
    "security": {},
    "history": {},
}


@pytest.fixture()
def state_file(tmp_path, monkeypatch):
    path = tmp_path / "house_state.json"

    def use(state):
        path.write_text(json.dumps(state), encoding="utf-8")
        monkeypatch.setattr(service, "HOUSE_STATE_PATH", str(path))
        return path

    return use


def test_summary_list_schema(state_file):
    state_file(LIST_STATE)
    out = service.house_summary()
    assert "1 lights on" in out
    assert "1 locks unlocked" in out
    assert "auto" in out


def test_summary_legacy_dict_schema(state_file):
    state_file(DICT_STATE)
    out = service.house_summary()
    assert "1 lights on" in out
    assert "0 locks unlocked" in out


def test_summary_empty_state_has_no_thermostat_crash(state_file):
    state_file({"devices": [], "energy": {}})
    out = service.house_summary()
    assert "0 lights on" in out
    assert "unknown" in out


def test_get_device_both_shapes(state_file):
    state_file(LIST_STATE)
    assert service.get_device("light-1")["name"] == "Ceiling"
    assert service.get_device("nope") is None
    state_file(DICT_STATE)
    assert service.get_device("living_light_1")["name"] == "Lamp"


def test_set_device_preserves_list_shape_and_normalizes_keys(state_file):
    path = state_file(LIST_STATE)
    res = service.set_device("light-1", {"brightness": 10, "on": True})
    assert res["ok"] is True
    assert res["device"]["state"]["power"] is True
    assert res["device"]["state"]["brightness"] == 10
    reloaded = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(reloaded["devices"], list)


def test_set_device_unknown_and_bad_patch(state_file):
    state_file(LIST_STATE)
    assert service.set_device("ghost", {"power": True})["ok"] is False
    bad = service.set_device("light-1", {"nonsense": 1})
    assert bad["ok"] is False


def test_set_room_state_matches_room_id(state_file):
    state_file(LIST_STATE)
    res = service.set_room_state("living", {"power": False})
    assert "light-1" in res["applied"]
    assert service.get_device("light-1")["state"]["power"] is False
    assert service.set_room_state("nowhere", {"power": True})["error"].startswith("Room")


def test_run_scene_list_and_dict_actions(state_file):
    state_file(LIST_STATE)
    res = service.run_scene("scene-1")
    assert res["ok"] is True
    assert res["applied"] == ["light-1"]
    assert service.get_device("light-1")["state"]["power"] is False
    state_file(DICT_STATE)
    res = service.run_scene("night")
    assert res["ok"] is True
    assert service.run_scene("ghost")["ok"] is False


def test_run_scene_skipped_contract_is_pairs(state_file):
    state_file(LIST_STATE)
    res = service.run_scene("scene-1")
    for entry in res["skipped"]:
        sid, _why = entry  # house_tools unpacks pairs; must not raise
        assert isinstance(sid, str)


def test_device_history_has_series(state_file):
    state_file(LIST_STATE)
    res = service.get_device_history("light-1", 5)
    assert len(res["series"]) == 5
    assert len(res["points"]) == 5
