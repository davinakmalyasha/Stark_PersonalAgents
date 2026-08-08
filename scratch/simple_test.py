"""Simple test: one task, check if files created, check token tracking."""
import os, sys, shutil
sys.path.insert(0, r'D:\MyProject\LangChain')
os.environ['DEEP_AGENTS_BUDGET_CAP'] = '5.00'

sandbox = r'D:\MyProject\TestProjectForAgent'
# Clean sandbox
for f in os.listdir(sandbox):
    path = os.path.join(sandbox, f)
    try:
        if os.path.isfile(path): os.remove(path)
        elif os.path.isdir(path) and f != '.deep_agents': shutil.rmtree(path)
    except: pass

from sync_manager import run_and_sync_graph
from state_sync import safe_get_state

task = 'Create a file called hello.py with a function greet() that returns hello world. Also create test_hello.py that imports and tests it.'
print('Task:', task[:80])

for snap in run_and_sync_graph(task, workspace_path=sandbox, chat_id='simple-test'):
    node = snap.get('active_node', '?')
    next_ag = snap.get('next_agent', '?')
    thoughts = snap.get('thoughts', {})
    dev = thoughts.get('developer', '')[:120]
    sup = thoughts.get('supervisor', '')[:120]
    code = snap.get('code', '') or snap.get('agent_report', '') or ''
    print('  [%s] next=%s | %s' % (node, next_ag, (dev or sup or code)[:100]))

# Check files
print()
for f in ['hello.py', 'test_hello.py', 'planning.md']:
    p = os.path.join(sandbox, f)
    if os.path.isfile(p):
        print('%s: EXISTS (%d bytes)' % (f, os.path.getsize(p)))
    else:
        print('%s: MISSING' % f)

# Check token usage
state = safe_get_state()
tu = state.get('token_usage', {})
print('\nToken: cost=$%.6f in=%d out=%d cache_hit=%d calls=%d' % (
    tu.get('total_cost',0), tu.get('total_input_tokens',0),
    tu.get('total_output_tokens',0), tu.get('total_cache_hit_tokens',0),
    len(tu.get('calls',[]))))
