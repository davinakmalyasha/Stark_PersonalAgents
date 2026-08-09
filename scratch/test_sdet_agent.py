import unittest
from unittest.mock import patch, MagicMock
from tools import _load_subagents, _SUBAGENTS_REGISTRY
from sdet_agent import SdetDiagnosis, SdetIssue, invoke_sdet, format_diagnosis_for_developer

class TestSdetAgent(unittest.TestCase):
    def setUp(self):
        _load_subagents()

    def test_sdet_registration(self):
        """Verify SDET subagent is registered and has the right tools."""
        self.assertIn("SDET", _SUBAGENTS_REGISTRY)
        sdet_spec = _SUBAGENTS_REGISTRY["SDET"]
        
        self.assertEqual(sdet_spec["name"], "SDET")
        self.assertIn("read_file", sdet_spec["tools"])
        self.assertIn("browser_navigate", sdet_spec["tools"])
        self.assertIn("browser_click", sdet_spec["tools"])
        self.assertIn("browser_fill", sdet_spec["tools"])
        self.assertIn("browser_type", sdet_spec["tools"])
        self.assertIn("browser_press", sdet_spec["tools"])
        self.assertIn("browser_screenshot", sdet_spec["tools"])
        self.assertIn("browser_extract", sdet_spec["tools"])
        self.assertIn("browser_close", sdet_spec["tools"])
        self.assertIn("run_command", sdet_spec["tools"])
        self.assertEqual(sdet_spec["model"], "deepseek:v4-flash")
        
        # Verify the prompt is large and loaded
        prompt = sdet_spec["system_prompt"]
        self.assertTrue(len(prompt) > 10000, f"Prompt length is too short: {len(prompt)}")

    def test_critic_aliases(self):
        """Verify that legacy 'Critic' and 'CodeCritic' aliases map to SDET."""
        self.assertIn("Critic", _SUBAGENTS_REGISTRY)
        self.assertIn("CodeCritic", _SUBAGENTS_REGISTRY)
        
        critic_spec = _SUBAGENTS_REGISTRY["Critic"]
        code_critic_spec = _SUBAGENTS_REGISTRY["CodeCritic"]
        
        self.assertEqual(critic_spec["name"], "Critic")
        self.assertEqual(code_critic_spec["name"], "CodeCritic")
        
        # Verify system prompts are equivalent to SDET system prompt
        sdet_prompt = _SUBAGENTS_REGISTRY["SDET"]["system_prompt"]
        self.assertEqual(critic_spec["system_prompt"], sdet_prompt)
        self.assertEqual(code_critic_spec["system_prompt"], sdet_prompt)

    def test_format_diagnosis(self):
        """Verify formatting SdetDiagnosis into human readable output."""
        diagnosis = SdetDiagnosis(
            status="FIX_REQUIRED",
            summary="Test failures detected.",
            root_cause="SQL error in users.",
            files_to_read=["app/db.py"],
            issues=[
                SdetIssue(
                    file="app/db.py",
                    line=45,
                    severity="critical",
                    category="security",
                    issue="SQL Injection vulnerability found in user login.",
                    fix="Use parameterized queries instead of string formatting.",
                    confidence=1.0
                )
            ]
        )
        
        formatted = format_diagnosis_for_developer(diagnosis)
        self.assertIn("[STAGE 3 --- SDET DIAGNOSIS]", formatted)
        self.assertIn("**Status:** FIX_REQUIRED", formatted)
        self.assertIn("**Root Cause:** SQL error in users.", formatted)
        self.assertIn("[CRITICAL] app/db.py:45", formatted)
        self.assertIn("**Category:** security | **Confidence:** 100%", formatted)

if __name__ == "__main__":
    unittest.main()
