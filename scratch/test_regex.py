import re

client_req = "Wait, pivot request: We must support token expiration checking. The validate function must check the `exp` claim in the payload against the current UTC timestamp and return False if expired. Update `jwt_utils.py` and the tests in `test_jwt_utils.py` to verify expiration handling."

clean_req = client_req.split("=== TASK PROGRESS")[0].strip()
is_plan_req = bool(re.search(r"(?:^|\s)/plan(?:\s|$|\b|,)", clean_req.lower()))
is_execution = clean_req.startswith("Execute the plan from planning.md for:")
is_fixing = False

is_write = any(kw in clean_req.lower() for kw in ["build", "create", "write", "code", "fix", "implement", "add", "change", "modify", "refactor", "edit", "update", "make", "support"])
is_exploration = not is_plan_req and not is_execution and not is_fixing and not is_write and any(kw in clean_req.lower() for kw in ["explore", "analyze", "read", "report", "find", "check"])

print("is_plan_req:", is_plan_req)
print("is_execution:", is_execution)
print("is_write:", is_write)
print("is_exploration:", is_exploration)
