import unittest
from unittest.mock import patch, MagicMock
import os

# Set dummy env var before importing tools
os.environ["GITHUB_TOKEN"] = "dummy_token"

from tools import execute_tool
from devops_verifier import (
    validate_yaml, validate_json, validate_dockerfile,
    detect_secret_leak, validate_kubernetes_manifest,
    validate_bash_script, verify_devops_files
)

class TestDevOpsVerifier(unittest.TestCase):
    def test_validate_yaml(self):
        good_yaml = "name: Test\njobs:\n  build:\n    runs-on: ubuntu-latest"
        bad_yaml = "name: Test\njobs:\n  build:\n  - runs-on: [invalid"
        self.assertEqual(validate_yaml(good_yaml), [])
        self.assertTrue(len(validate_yaml(bad_yaml)) > 0)

    def test_validate_json(self):
        good_json = '{"name": "test", "private": true}'
        bad_json = '{"name": "test", "private": }'
        self.assertEqual(validate_json(good_json), [])
        self.assertTrue(len(validate_json(bad_json)) > 0)

    def test_validate_dockerfile(self):
        # 1. Missing tag and USER
        df_bad = "FROM node\nCOPY . .\nCMD [\"node\", \"index.js\"]"
        errors = validate_dockerfile(df_bad)
        self.assertTrue(any("Base image" in e for e in errors))
        self.assertTrue(any("USER directive" in e for e in errors))

        # 2. Forbidden :latest tag
        df_latest = "FROM node:latest\nUSER node"
        errors = validate_dockerfile(df_latest)
        self.assertTrue(any("forbidden ':latest'" in e for e in errors))

        # 3. Correct Dockerfile
        df_good = "FROM node:20-bookworm-slim\nUSER node\nWORKDIR /app"
        self.assertEqual(validate_dockerfile(df_good), [])

    def test_detect_secret_leak(self):
        # 1. Obvious token leakage
        leak_content = "ghp_1234567890abcdefghijklmnopqrstuvwxyzABCD"
        errors = detect_secret_leak(leak_content)
        self.assertTrue(len(errors) > 0)
        self.assertTrue(any("GitHub Token" in e for e in errors))

        # 2. Hardcoded password/secret credential
        cred_leak = 'db_password = "supersecret_password_1234!"'
        errors = detect_secret_leak(cred_leak)
        self.assertTrue(len(errors) > 0)
        self.assertTrue(any("Potential hardcoded credential" in e for e in errors))

        # 3. Dummy variables ignored
        dummy_content = 'db_password = "your_password_here"'
        errors = detect_secret_leak(dummy_content)
        self.assertEqual(errors, [])

    def test_validate_kubernetes_manifest(self):
        # 1. Invalid manifest missing resources & securityContext
        k8s_bad = """
apiVersion: apps/v1
kind: Deployment
metadata:
  name: bad-deploy
spec:
  template:
    spec:
      containers:
        - name: app
          image: app:1.0.0
"""
        errors = validate_kubernetes_manifest(k8s_bad)
        self.assertTrue(len(errors) > 0)
        self.assertTrue(any("missing resource limits" in e for e in errors))
        self.assertTrue(any("missing securityContext" in e for e in errors))

        # 2. Correct manifest
        k8s_good = """
apiVersion: apps/v1
kind: Deployment
metadata:
  name: good-deploy
spec:
  template:
    spec:
      containers:
        - name: app
          image: app:1.0.0
          resources:
            requests:
              memory: "64Mi"
              cpu: "250m"
            limits:
              memory: "128Mi"
              cpu: "500m"
          securityContext:
            runAsNonRoot: true
            allowPrivilegeEscalation: false
"""
        self.assertEqual(validate_kubernetes_manifest(k8s_good), [])

    def test_validate_bash_script(self):
        # 1. Missing shebang
        sh_bad = "echo 'hello'\n"
        errors = validate_bash_script(sh_bad)
        self.assertTrue(any("missing a shebang" in e for e in errors))

        # 2. Unbalanced brackets
        sh_unbalanced = "#!/bin/bash\nif [ $x -eq 1 ; then\n  echo 'bad'\nfi\n"
        errors = validate_bash_script(sh_unbalanced)
        self.assertTrue(any("Unbalanced brackets" in e for e in errors))

        # 3. Correct script
        sh_good = "#!/bin/bash\nif [ $x -eq 1 ]; then\n  echo 'good'\nfi\n"
        self.assertEqual(validate_bash_script(sh_good), [])

class TestDevOpsGitHubTools(unittest.TestCase):
    @patch("requests.post")
    @patch("requests.get")
    def test_github_create_issue(self, mock_get, mock_post):
        # Mock /user to get login
        mock_user = MagicMock()
        mock_user.status_code = 200
        mock_user.json.return_value = {"login": "test-owner"}
        mock_get.return_value = mock_user

        # Mock issue creation post
        mock_issue = MagicMock()
        mock_issue.status_code = 201
        mock_issue.json.return_value = {
            "number": 42,
            "html_url": "https://github.com/test-owner/test-repo/issues/42"
        }
        mock_post.return_value = mock_issue

        res = execute_tool("github_create_issue", {
            "repo": "test-repo",
            "title": "Bug report",
            "body": "Something is broken."
        })
        self.assertIn("Success: Issue #42", res)
        mock_post.assert_called_once()

if __name__ == "__main__":
    unittest.main()
