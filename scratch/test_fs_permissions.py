import unittest
import os
from tools import active_permissions, read_file, write_file, edit_file, list_files, search_code, to_virtual_path
from deepagents.middleware.filesystem import FilesystemPermission

class TestFSPermissions(unittest.TestCase):
    def test_to_virtual_path(self):
        # Test virtual mapping of physical paths
        self.assertEqual(to_virtual_path("/workspace/file.py"), "/workspace/file.py")
        
    def test_allow_deny_permissions(self):
        rules = [
            FilesystemPermission(
                operations=["write"],
                paths=["/workspace/restricted/**"],
                mode="deny"
            )
        ]
        token = active_permissions.set(rules)
        try:
            with self.assertRaises(PermissionError):
                write_file("/workspace/restricted/secret.txt", "secret content")
            with self.assertRaises(PermissionError):
                edit_file("/workspace/restricted/secret.txt", "old", "new")
        finally:
            active_permissions.reset(token)
            
    def test_interrupt_permission(self):
        rules = [
            FilesystemPermission(
                operations=["read"],
                paths=["/workspace/sensitive/**"],
                mode="interrupt"
            )
        ]
        token = active_permissions.set(rules)
        try:
            with self.assertRaises(PermissionError) as context:
                read_file("/workspace/sensitive/data.txt")
            self.assertIn("Permission denied", str(context.exception))
        finally:
            active_permissions.reset(token)

if __name__ == '__main__':
    unittest.main()
