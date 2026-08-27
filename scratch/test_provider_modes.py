"""Provider-mode switch tests: matrix resolution, ladders, llm.py transport
integration, persisted-file migration, and API validation."""

import json
import os
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


class TestMatrixShape(unittest.TestCase):
    def test_all_modes_define_every_tier(self):
        from llm.provider_modes import MODE_MATRIX, MODES

        self.assertEqual(MODES, ("free", "deepseek", "opencode", "command"))
        for mode in MODES:
            for tier in ("orchestrator", "workspace", "specialist"):
                self.assertIn(tier, MODE_MATRIX[mode], f"{mode}/{tier} missing")

    def test_tier_mapping(self):
        from llm.provider_modes import get_tier

        self.assertEqual(get_tier("Orchestrator"), "orchestrator")
        self.assertEqual(get_tier("Developer"), "workspace")
        self.assertEqual(get_tier("DeveloperFixing"), "workspace")
        self.assertEqual(get_tier("General"), "workspace")
        self.assertEqual(get_tier("House"), "workspace")
        self.assertEqual(get_tier("UI"), "specialist")
        self.assertEqual(get_tier("Deploy"), "specialist")
        self.assertEqual(get_tier(""), "specialist")


class TestRolePlans(unittest.TestCase):
    """The 12 approved matrix cells."""

    def test_free_mode(self):
        from llm.provider_modes import get_role_plan

        orch = get_role_plan("Orchestrator", mode="free")
        self.assertEqual(orch["backend"], "free")
        self.assertFalse(orch["think_bias"])
        work = get_role_plan("Developer", mode="free")
        self.assertEqual(work["backend"], "free")
        self.assertTrue(work["think_bias"])
        spec = get_role_plan("UI", mode="free")
        self.assertEqual(spec["backend"], "free")

    def test_deepseek_mode(self):
        from llm.provider_modes import get_role_plan

        orch = get_role_plan("Orchestrator", mode="deepseek")
        self.assertEqual(orch["backend"], "paid")
        self.assertEqual(orch["model"], "deepseek-v4-flash-vision-exp")
        self.assertEqual(orch["iter_effort"], "disabled")
        work = get_role_plan("General", mode="deepseek")
        self.assertEqual(work["model"], "deepseek-v4-flash")
        self.assertEqual(work["iter_effort"], "high")
        spec = get_role_plan("Deploy", mode="deepseek")
        self.assertEqual(spec["backend"], "free")

    def test_opencode_mode_matches_deepseek(self):
        from llm.provider_modes import get_role_plan

        for role in ("Orchestrator", "House"):
            plan = get_role_plan(role, mode="opencode")
            ref = get_role_plan(role, mode="deepseek")
            self.assertEqual(plan["model"], ref["model"])
            self.assertEqual(plan["iter_effort"], ref["iter_effort"])

    def test_command_mode(self):
        from llm.provider_modes import get_role_plan

        orch = get_role_plan("Orchestrator", mode="command")
        self.assertEqual(orch["backend"], "paid")
        self.assertEqual(orch["iter_effort"], "low")
        self.assertEqual(orch["model"], "laguna-s-2.1-free")
        work = get_role_plan("Developer", mode="command")
        self.assertEqual(work["iter_effort"], "high")
        self.assertEqual(work["model"], "deepseek-v4-flash")
        spec = get_role_plan("UI", mode="command")
        self.assertEqual(spec["backend"], "paid")
        self.assertEqual(spec["model"], "laguna-s-2.1-free")

    def test_command_model_env_override(self):
        from llm.provider_modes import get_role_plan

        with patch.dict(os.environ, {"COMMAND_CODE_MODEL_ORCH": "custom/orch"}):
            orch = get_role_plan("Orchestrator", mode="command")
        self.assertEqual(orch["model"], "custom/orch")


class TestSpecialistResolution(unittest.TestCase):
    def _resolve(self, mode, spec_backend="free", env=""):
        from llm import provider_modes

        with patch.object(provider_modes, "resolve_mode", return_value=mode), \
             patch.dict(os.environ, {"SUBAGENT_MODEL_BACKEND": env}):
            return provider_modes.specialist_backend("UI", spec_backend)

    def test_env_wins_even_in_command_mode(self):
        self.assertEqual(self._resolve("command", env="groq"), "groq")

    def test_mode_entries(self):
        self.assertEqual(self._resolve("free"), "free")
        self.assertEqual(self._resolve("deepseek"), "free")
        self.assertEqual(self._resolve("opencode"), "free")
        self.assertEqual(self._resolve("command"), "paid")

    def test_spec_fallback_when_mode_has_no_entry_backend(self):
        # Defensive: a hypothetical mode without a specialist entry falls back
        # to the subagents.json spec value.
        from llm import provider_modes

        with patch.object(provider_modes, "resolve_mode", return_value="command"), \
             patch.dict(os.environ, {"SUBAGENT_MODEL_BACKEND": ""}), \
             patch.dict(provider_modes.MODE_MATRIX, {"command": {"specialist": {}}}):
            self.assertEqual(
                provider_modes.specialist_backend("UI", "deepseek"), "deepseek"
            )

    def test_specialist_model_only_in_command_mode(self):
        from llm import provider_modes

        with patch.object(provider_modes, "resolve_mode", return_value="command"):
            self.assertEqual(provider_modes.specialist_model(), "laguna-s-2.1-free")
            with patch.dict(os.environ, {"COMMAND_CODE_MODEL_LIGHT": "x/light"}):
                self.assertEqual(provider_modes.specialist_model(), "x/light")
        with patch.object(provider_modes, "resolve_mode", return_value="deepseek"):
            self.assertIsNone(provider_modes.specialist_model())
            self.assertEqual(provider_modes.specialist_model("spec-m"), "spec-m")


class TestLlmIntegration(unittest.TestCase):
    def setUp(self):
        from sync.state_sync import reset_shared_state, safe_update_state

        reset_shared_state()
        safe_update_state({"llm_provider": "", "selected_temp": None} or {"llm_provider": ""})

    def _set_mode(self, mode):
        from sync.state_sync import safe_update_state

        safe_update_state({"llm_provider": mode})

    def test_command_row_resolves_org_slugs(self):
        from llm.llm import _map_model, _resolve_llm_provider

        self._set_mode("command")
        with patch.dict(os.environ, {
            "COMMAND_CODE_API_KEY": "user_test",
            "COMMAND_CODE_BASE_URL": "http://127.0.0.1:4891/v1",
        }):
            cfg = _resolve_llm_provider()
        self.assertEqual(cfg["base"], "http://127.0.0.1:4891/v1")
        self.assertEqual(cfg["key"], "user_test")
        self.assertEqual(cfg["flash"], "deepseek/deepseek-v4-flash")
        self.assertEqual(cfg["pro"], "deepseek/deepseek-v4-pro")
        self.assertEqual(cfg["orch"], "laguna-s-2.1-free")
        # org-scoped literals pass through untouched
        self.assertEqual(_map_model("laguna-s-2.1-free"), "laguna-s-2.1-free")
        self.assertEqual(_map_model("deepseek/deepseek-v4-flash"), "deepseek/deepseek-v4-flash")
        # canonical vision name maps into the command slot
        self.assertEqual(_map_model("deepseek-v4-flash-vision-exp"),
                         "deepseek/deepseek-v4-flash-vision-exp")

    def test_deepseek_row_unchanged(self):
        from llm.llm import _map_model, _resolve_llm_provider

        self._set_mode("deepseek")
        cfg = _resolve_llm_provider()
        self.assertEqual(cfg["base"], os.getenv("DEEPSEEK_BASE_URL", "https://api.deepseek.com"))
        self.assertEqual(_map_model("deepseek-v4-flash"), "deepseek-v4-flash")
        self.assertEqual(cfg["vision"], os.getenv("DEEPSEEK_MODEL_VISION", "deepseek-v4-flash-vision-exp"))

    def test_persisted_file_migration(self):
        import llm.llm as L

        with tempfile.TemporaryDirectory() as tmp:
            new_dir = os.path.join(tmp, "root", ".deep_agents")
            legacy_dir = os.path.join(tmp, "llmmod", ".deep_agents")
            os.makedirs(new_dir)
            os.makedirs(legacy_dir)
            legacy_path = os.path.join(legacy_dir, "llm_provider.json")
            with open(legacy_path, "w", encoding="utf-8") as f:
                json.dump({"provider": "free"}, f)
            with patch.object(L, "_LLM_PROVIDER_FILE", os.path.join(new_dir, "llm_provider.json")), \
                 patch.object(L, "_LEGACY_LLM_PROVIDER_FILE", legacy_path):
                self.assertEqual(L._load_persisted_provider(), "free")
                # migrated forward + retired
                self.assertTrue(os.path.isfile(os.path.join(new_dir, "llm_provider.json")))
                self.assertTrue(os.path.isfile(legacy_path + ".migrated"))
                self.assertFalse(os.path.isfile(legacy_path))

    @patch.dict(os.environ, {"DEEPSEEK_API_KEY": "sk-test"})
    def test_client_effort_mapping(self):
        from sync.state_sync import safe_update_state

        safe_update_state({"llm_provider": "deepseek"})
        from llm.llm import _get_deepseek_client

        base = dict(model="deepseek-v4-flash", temp=0.2)
        c = _get_deepseek_client(reasoning_effort="disabled", **base)
        self.assertEqual(c.extra_body, {"thinking": {"type": "disabled"}})
        c = _get_deepseek_client(reasoning_effort="low", **base)
        self.assertEqual(c.extra_body.get("thinking", {}).get("type"), "enabled")
        self.assertEqual(c.reasoning_effort, "low")
        c = _get_deepseek_client(reasoning_effort="max", **base)
        self.assertEqual(c.reasoning_effort, "high")
        self.assertEqual(c.extra_body["thinking"]["budget_tokens"], 32768)


class TestApiEndpoints(unittest.TestCase):
    def setUp(self):
        try:
            from fastapi.testclient import TestClient

            from server.api_server import app

            # TestClient requests are not socket-loopback; treat them as local.
            self._loopback = patch("remote_auth.is_loopback", lambda request: True)
            self._loopback.start()
            self.client = TestClient(app)
        except Exception:
            self.skipTest("fastapi TestClient unavailable")

    def tearDown(self):
        self._loopback.stop()
        from sync.state_sync import reset_shared_state, safe_update_state

        reset_shared_state()
        safe_update_state({"llm_provider": ""})

    @patch.dict(os.environ, {"DEEPSEEK_API_KEY": "sk-test"})
    def test_post_rejects_invalid_and_accepts_four_modes(self):
        import llm.llm as L

        r = self.client.post("/api/provider", json={"provider": "bogus"})
        self.assertEqual(r.status_code, 400)
        with patch.object(L, "_save_persisted_provider", lambda p: None), \
             patch.dict(os.environ, {"DEEPSEEK_API_KEY": "sk-test"}):
            for mode in ("free", "deepseek", "opencode", "command"):
                r = self.client.post("/api/provider", json={"provider": mode})
                self.assertEqual(r.status_code, 200, mode)
                self.assertTrue(r.json()["ok"])

    @patch.dict(os.environ, {"DEEPSEEK_API_KEY": "sk-test"})
    def test_get_reports_matrix(self):
        from sync.state_sync import safe_update_state

        safe_update_state({"llm_provider": "command"})
        r = self.client.get("/api/provider")
        self.assertEqual(r.status_code, 200)
        data = r.json()
        self.assertEqual(data["provider"], "command")
        self.assertEqual(data["modes"], ["free", "deepseek", "opencode", "command"])
        for tier in ("orchestrator", "workspace", "specialist"):
            self.assertIn(tier, data["matrix"])


if __name__ == "__main__":
    unittest.main()
