"""AddOnTool browser tools.

The browser command bridge and browser primitives live here; tools.py
re-exports them so TOOL_MAP and execute_tool retain their existing surface.
"""
import os
import subprocess
import time

def browser_navigate(url: str, wait_ms: int = 5000) -> str:
    """Open a URL in a headless browser and return the page content (accessibility snapshot).

    Uses agent-browser (headless Chromium). Handles JavaScript-rendered pages,
    SPAs, and complex web apps. Returns the accessibility tree with @ref selectors
    that can be used with browser_extract.

    Best for: JavaScript-rendered pages, SPAs, pages needing login, complex web apps.
    Slower than a plain GET but handles everything.

    Args:
        url: The full URL to navigate to.
        wait_ms: Milliseconds to wait for page load (default 5000).
    """
    if not url.startswith(("http://", "https://")):
        url = "https://" + url
    import time
    # Open page
    nav = _browser_cmd(["open", url], timeout_sec=30)
    if nav.startswith("("):
        return nav
    # Let page settle
    time.sleep(min(wait_ms / 1000, 3))
    # Get snapshot + title
    snapshot = _browser_cmd(["snapshot", "-i", "--max-output", "15000"], timeout_sec=20)
    title = _browser_cmd(["get", "title"], timeout_sec=5)
    title_str = title if "empty" not in title.lower() and "error" not in title.lower() else ""
    return f"[BROWSER] {url}\nTitle: {title_str}\n\nAccessibility Tree:\n{snapshot[:15000]}"

def _browser_cmd(args: list[str], timeout_sec: int = 30) -> str:
    """Run an agent-browser CLI command. Daemon starts/stops automatically."""
    import subprocess, os
    agent_browser = "agent-browser"
    if os.name == "nt":
        npm_cmd = os.path.expanduser(r"~\AppData\Roaming\npm\agent-browser.cmd")
        if os.path.isfile(npm_cmd):
            agent_browser = npm_cmd
    try:
        r = subprocess.run([agent_browser] + args, capture_output=True, text=True, timeout=timeout_sec)
        return r.stdout.strip() or "(empty)"
    except FileNotFoundError:
        return "(agent-browser not found. Run: npm i -g agent-browser)"
    except subprocess.TimeoutExpired:
        return f"(timed out after {timeout_sec}s)"
    except Exception as e:
        return f"(error: {e})"

def browser_extract(what: str = "text", selector: str = "") -> str:
    """Extract content from the current browser page.

    Args:
        what: What to extract â€” 'text' (default), 'html', 'value', 'title', 'url'.
        selector: Optional CSS selector or @ref from snapshot. Empty = entire page.
    """
    valid = {"text", "html", "value", "title", "url", "attr"}
    if what not in valid:
        return f"(invalid: '{what}'. Valid: {', '.join(sorted(valid))})"
    if what in ("title", "url"):
        return _browser_cmd(["get", what], timeout_sec=5)
    if selector:
        return _browser_cmd(["get", what, selector], timeout_sec=15)
    return _browser_cmd(["get", what, "body"], timeout_sec=15)

def browser_screenshot(path: str = "") -> str:
    """Take a screenshot of the current browser page.

    Saves the PNG to disk AND queues it for inline vision injection: the agent
    loop drains tools.vision_channel after this ToolMessage and appends the
    image as a user message, so a vision-capable model can SEE the page.

    Args:
        path: Optional file path to save the screenshot. Auto-named if empty.
    """
    import os
    if not path:
        import uuid
        screenshot_dir = os.path.join(os.environ.get("TEMP", "."), "agent-screenshots")
        os.makedirs(screenshot_dir, exist_ok=True)
        path = os.path.join(screenshot_dir, f"screenshot_{uuid.uuid4().hex[:8]}.png")
    result = _browser_cmd(["screenshot", "--full", path], timeout_sec=20)
    if os.path.isfile(path):
        size = os.path.getsize(path)
        try:
            from tools.vision_channel import queue_image_file
            queued = queue_image_file(path, note=f"Browser screenshot ({size} bytes)")
        except Exception:
            queued = False
        inline = " — image attached inline (vision)" if queued else ""
        return f"[SCREENSHOT] Saved to {path} ({size} bytes){inline}"
    return f"(screenshot result: {result})"

def browser_click(selector: str) -> str:
    """Click an element using CSS selector or snapshot @ref tag."""
    return _browser_cmd(["click", selector], timeout_sec=15)

def browser_fill(selector: str, text: str) -> str:
    """Clear and fill an input element."""
    return _browser_cmd(["fill", selector, text], timeout_sec=15)

def browser_close() -> str:
    """Close the browser daemon and free resources."""
    _browser_cmd(["close", "--all"], timeout_sec=10)
    return "Browser closed."

def browser_act(action: str = "extract", selector: str = "", text: str = "",
                key: str = "", what: str = "text", path: str = "",
                url: str = "", wait_ms: int = 5000) -> str:
    """Unified browser tool: one entry point for navigate / click / fill /
    type / press / extract / screenshot / close. Routes to the individual
    browser primitives so the agent surface stays minimal (one schema, one
    action enum) while all capabilities remain available."""
    a = (action or "extract").strip().lower()
    if a == "navigate":
        return browser_navigate(url, wait_ms)
    if a == "click":
        return browser_click(selector)
    if a == "fill":
        return browser_fill(selector, text)
    if a == "type":
        return browser_type(selector, text)
    if a == "press":
        return browser_press(key)
    if a == "extract":
        return browser_extract(what, selector)
    if a == "screenshot":
        return browser_screenshot(path)
    if a == "close":
        return browser_close()
    return (f"Error: unknown browser_act action '{action}'. "
            f"Valid: navigate, click, fill, type, press, extract, screenshot, close")
