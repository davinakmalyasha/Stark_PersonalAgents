import sys
from typing import Any, List
import quickjs_rs

def run_js_in_sandbox(code: str, timeout_ms: int = 500, extra_funcs: dict = None) -> str:
    """
    Run Javascript code locally in an in-memory sandboxed QuickJS context.
    Captures console.log statements and enforces a strict timeout.
    """
    if len(code) > 20000:
        return "Error: JS code exceeds 20KB limit."

    logs: List[str] = []

    def js_log(*args: Any) -> None:
        logs.append(" ".join(str(arg) for arg in args))

    try:
        # Initialize the QuickJS runtime with limits (e.g. 10MB heap limit)
        rt = quickjs_rs.Runtime(memory_limit=10 * 1024 * 1024)
        # new_context takes a timeout in float seconds
        ctx = rt.new_context(timeout=timeout_ms / 1000.0)

        # Register the log function in the JS context
        ctx.register("js_log", js_log)

        # Initialize the console.log wrapper
        # We append '; undefined' to avoid marshaling the console object back to Python
        ctx.eval("var console = { log: js_log }; undefined;")

        # Register extra functions if provided
        if extra_funcs:
            for name, func in extra_funcs.items():
                ctx.register(name, func)
            
            # Inject standard JS-side wrappers to make the registered python functions
            # look like standard JS functions (e.g. read_file, write_file, edit_file, etc.)
            wrapper_code = ""
            for name in extra_funcs.keys():
                if name.startswith("_py_"):
                    js_name = name[4:]
                    wrapper_code += f"var {js_name} = function(...args) {{ return {name}(...args); }};\n"
            if wrapper_code:
                ctx.eval(wrapper_code)

        # Evaluate the user's code
        res = ctx.eval(code)

        if logs:
            return "\n".join(logs)

        if res is None:
            return "(no output)"

        return str(res)

    except quickjs_rs.errors.TimeoutError:
        return f"Error: Javascript execution timed out after {timeout_ms}ms."
    except Exception as e:
        return f"Runtime Error:\n{e}"
