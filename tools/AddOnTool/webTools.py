"""AddOnTool web-fetch and web-search tools.

The tools.py facade re-exports these functions for compatibility.
"""

def web_fetch(url: str, max_chars: int = 10000) -> str:
    """Fetch a URL via plain HTTP GET and return text content. No browser overhead.

    Best for: documentation, API references, blog posts, static pages.
    NOT for: JavaScript-rendered SPAs or pages requiring login.

    Args:
        url: The full URL to fetch (https://...).
        max_chars: Maximum characters to return (default 10000).
    """
    if not url.startswith(("http://", "https://")):
        url = "https://" + url
    try:
        import ssl
        import urllib.request
        # Python installations on Windows do not always use the same CA store
        # as the browser. Prefer certifi's maintained CA bundle for research
        # requests so valid public HTTPS sites are not rejected as untrusted.
        try:
            import certifi
            ssl_context = ssl.create_default_context(cafile=certifi.where())
        except Exception:
            ssl_context = ssl.create_default_context()
        req = urllib.request.Request(url, headers={
            "User-Agent": "Mozilla/5.0 (compatible; ResearchBot/1.0)",
            "Accept": "text/html,text/plain,*/*",
        })
        with urllib.request.urlopen(req, timeout=15, context=ssl_context) as resp:
            raw = resp.read()
            # Try UTF-8 first, fallback to detected charset
            content_type = resp.headers.get("Content-Type", "")
            charset = "utf-8"
            if "charset=" in content_type:
                charset = content_type.split("charset=")[-1].split(";")[0].strip()
            try:
                text = raw.decode(charset)
            except (UnicodeDecodeError, LookupError):
                text = raw.decode("utf-8", errors="replace")

        # Strip HTML tags for a clean text version
        import re
        text = re.sub(r"<script[^>]*>.*?</script>", "", text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r"<style[^>]*>.*?</style>", "", text, flags=re.DOTALL | re.IGNORECASE)
        text = re.sub(r"<[^>]+>", " ", text)
        text = re.sub(r"\s+", " ", text).strip()

        if not text:
            return "(page returned no text content)"

        if len(text) > max_chars:
            text = text[:max_chars] + f"\n... (truncated, full length: {len(text)} chars)"

        return f"[WEB] {url}\n{text[:max_chars]}"
    except urllib.error.HTTPError as e:
        return f"(HTTP {e.code}: {e.reason} for {url})"
    except urllib.error.URLError as e:
        return f"(connection error: {e.reason})"
    except Exception as e:
        return f"(fetch error: {e})"

def web_search(query, max_results=5):
    """Search the web via DuckDuckGo."""
    import urllib.parse
    encoded = urllib.parse.quote(query)
    url = "https://html.duckduckgo.com/html/?q=" + encoded
    try:
        result = web_fetch(url, max_chars=8000)
        import re
        snippets = re.findall(
            r'<a[^>]*class="result__a"[^>]*>(.*?)</a>.*?'
            r'<a[^>]*class="result__snippet"[^>]*>(.*?)</a>',
            result, re.DOTALL
        )
        if snippets:
            output = 'Web search: ' + chr(34) + query + chr(34)
            for i, (title, snippet) in enumerate(snippets[:max_results], 1):
                clean_title = re.sub(r'<[^>]+>', '', title).strip()
                clean_snippet = re.sub(r'<[^>]+>', '', snippet).strip()
                output += chr(10) + str(i) + '. ' + clean_title + chr(10) + '   ' + clean_snippet
            return output
        return "Search: " + chr(34) + query + chr(34) + chr(10) + result[:10000]
    except Exception as e:
        return "Search error: " + str(e)
