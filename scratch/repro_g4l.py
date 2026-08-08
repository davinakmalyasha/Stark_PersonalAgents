import subprocess
d = r'C:\Users\MY LENOVO\AppData\Local\Temp\eval_g4l-stateful-crash_jugerlgd'
code = "const u=require('./utils.js'); console.log('open=',u.open('file.txt'),'read=',JSON.stringify(u.read()))"
r = subprocess.run(['node', '-e', code], cwd=d, capture_output=True, text=True)
print('direct:', r.stdout, r.stderr)
r2 = subprocess.run(['node', 'test_utils.js'], cwd=d, capture_output=True, text=True)
print('mirror rc=', r2.returncode)
print(r2.stdout[-300:])
print(r2.stderr[-400:])
