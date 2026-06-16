"""
DEPRECATED — legacy DevSecOps node.

The live DevSecOps execution path is the generic subagent loop in tools.py
(task("DevSecOps", ...)), which validates written files via devops_verifier
(the [HOST VALIDATION] block). This module's devops_node was dead code with a
third, divergent tool allow-list — removed. Only the template alias is kept
for any legacy imports.
"""
from devops_prompt import _STATIC_SYSTEM_TEMPLATE

_DEVOPS_SYSTEM_TEMPLATE = _STATIC_SYSTEM_TEMPLATE
