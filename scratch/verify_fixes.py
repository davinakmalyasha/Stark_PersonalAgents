"""Quick verification of all fixes."""
import sys
sys.path.insert(0, r'D:\MyProject\LangChain')

# 1. Code cache disabled (fully removed)
# from code_cache import get_cached_code, set_cached_code
# set_cached_code('test task', 'some code')
# assert get_cached_code('test task') is None, 'Cache should be disabled by default'
# print('[OK] Code cache disabled')

# 2. Orchestrator uses flash-first (pro fallback)
from llm_config import TASK_CATEGORIES, get_task_category
assert get_task_category('Developer') == 'orchestrator'
assert get_task_category('DeveloperFixing') == 'orchestrator'
assert TASK_CATEGORIES['orchestrator'][0]['model'] == 'deepseek-v4-flash'
print('[OK] Orchestrator uses flash-first')

# 3. XML parser
from developer_agent import _parse_xml_tool_call, parse_all_tool_calls

# Format A: <function> wrapper
r = _parse_xml_tool_call('<function><tool_name>read_file</tool_name><args><param>file_path</param><value>/test</value></args></function>')
assert r == [{'tool': 'read_file', 'args': {'file_path': '/test'}}], f'Format A failed: {r}'
print('[OK] XML Format A (<function>)')

# Format B: self-closing
r = _parse_xml_tool_call('<read_file file_path="test.html" />')
assert r == [{'tool': 'read_file', 'args': {'file_path': 'test.html'}}], f'Format B failed: {r}'
print('[OK] XML Format B (self-closing)')

# Format B2: wrapping
r = _parse_xml_tool_call('<read_file><file_path>x.html</file_path><offset>5</offset></read_file>')
assert r == [{'tool': 'read_file', 'args': {'file_path': 'x.html', 'offset': 5}}], f'Format B2 failed: {r}'
print('[OK] XML Format B2 (wrapping)')

# Format C: DSML
r = _parse_xml_tool_call('||DSML||tool_calls>\n||DSML||invoke name="list_files">\n||DSML||parameter name="path" string="true">D:/test</||DSML||parameter>\n</||DSML||invoke>\n</||DSML||tool_calls>')
assert len(r) == 1 and r[0]['tool'] == 'list_files' and r[0]['args']['path'] == 'D:/test', f'Format C failed: {r}'
print('[OK] DSML Format C')

# 4. parse_all_tool_calls covers all
assert len(parse_all_tool_calls('```tool\n{"tool": "list_files", "args": {"path": "."}}\n```')) == 1
assert len(parse_all_tool_calls('<function><tool_name>write_file</tool_name><args><param>file_path</param><value>t.py</value><param>content</param><value>print(1)</value></args></function>')) == 1
assert len(parse_all_tool_calls('<read_file file_path="test.html" />')) == 1
print('[OK] parse_all_tool_calls covers all 3 formats')

# 5. state reset works
from state_sync import reset_shared_state, safe_get_state, shared_state
shared_state['token_usage']['total_cost'] = 999
reset_shared_state()
assert safe_get_state()['token_usage']['total_cost'] == 0
print('[OK] reset_shared_state')

# 6. Test real gauntlet output samples
task3_output = "<function>\n<tool_name>read_file</tool_name>\n<args>\n<param>file_path</param>\n<value>D:\\MyProject\\GauntletSandbox\\index.html</value>\n</args>\n</function>"
parsed = parse_all_tool_calls(task3_output)
assert len(parsed) == 1, f'Task 3 output not parsed: {parsed}'
assert parsed[0]['tool'] == 'read_file'
print('[OK] Real Task 3 output parses correctly')

task5_output = '<||DSML||tool_calls>\n<||DSML||invoke name="list_files">\n<||DSML||parameter name="path" string="true">D:\\MyProject\\GauntletSandbox</||DSML||parameter>\n<||DSML||parameter name="recursive" string="true">true</||DSML||parameter>\n</||DSML||invoke>\n</||DSML||tool_calls>'
parsed = parse_all_tool_calls(task5_output)
assert len(parsed) == 1, f'Task 5 output not parsed: {parsed}'
assert parsed[0]['tool'] == 'list_files'
print('[OK] Real Task 5 DSML output parses correctly')

task7_output = "Let me start by reading the existing files to understand the current state of the project.\n\nI'll begin by reading the existing project files to understand the current structure.\n\n<read_file>\n<file_path>index.html</file_path>\n</read_file>"
parsed = parse_all_tool_calls(task7_output)
assert len(parsed) == 1, f'Task 7 output not parsed: {parsed}'
assert parsed[0]['tool'] == 'read_file'
print('[OK] Real Task 7 mixed output parses correctly')

print('\n=== ALL 10 CHECKS PASSED ===')
