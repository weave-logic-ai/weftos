#!/usr/bin/env python3
"""Dev proxy server for clawft-wasm browser test harness.

Serves static files from a directory and proxies /proxy/* requests to avoid
browser CORS restrictions during development. API requests like:

    POST /proxy/https://openrouter.ai/api/v1/chat/completions

are forwarded to the real URL with all headers intact, then the response is
returned with permissive CORS headers.

Usage:
    python3 dev_server.py [PORT] [DIRECTORY]
"""

import http.server
import sys
import os
import urllib.parse
import urllib.request
import urllib.error

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
DIRECTORY = sys.argv[2] if len(sys.argv) > 2 else "."

# Only these LLM API hosts may be proxied (HTTPS only), so the dev server is not an
# open proxy (CodeQL py/full-ssrf). Extend with DEV_PROXY_ALLOW="host1,host2".
ALLOWED_HOSTS = {
    "openrouter.ai",
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "api.groq.com",
    "api.mistral.ai",
}
ALLOWED_HOSTS |= {
    h.strip().lower() for h in os.environ.get("DEV_PROXY_ALLOW", "").split(",") if h.strip()
}
# Bind to loopback unless DEV_SERVER_BIND says otherwise.
BIND = os.environ.get("DEV_SERVER_BIND", "127.0.0.1")


def proxy_target_allowed(url):
    """True when `url` is https:// to an allowed host, with no userinfo and the default port."""
    try:
        u = urllib.parse.urlsplit(url)
    except ValueError:
        return False
    if u.scheme != "https" or u.username or u.password:
        return False
    if u.port not in (None, 443):
        return False
    return (u.hostname or "").lower() in ALLOWED_HOSTS


# Headers to forward from browser request to upstream API.
FORWARD_HEADERS = [
    "Authorization",
    "Content-Type",
    "Accept",
    "X-Title",
    "HTTP-Referer",
    "anthropic-version",
    "anthropic-dangerous-direct-browser-access",
]

# Headers to NOT copy from upstream response.
# - hop-by-hop headers (connection, transfer-encoding, etc.)
# - content-encoding / content-length: urllib auto-decompresses gzip/deflate
#   but keeps the original headers; we must drop them and set our own
#   Content-Length for the decompressed body.
SKIP_RESPONSE_HEADERS = {
    "transfer-encoding",
    "content-encoding",
    "content-length",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
}


class ProxyHandler(http.server.SimpleHTTPRequestHandler):
    """Serves static files and proxies /proxy/* requests."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=DIRECTORY, **kwargs)

    # -- CORS preflight -------------------------------------------------------

    def do_OPTIONS(self):
        if self.path.startswith("/proxy/"):
            self.send_response(204)
            self._cors_headers()
            self.end_headers()
        else:
            super().do_OPTIONS()

    # -- Proxy dispatch -------------------------------------------------------

    def do_GET(self):
        if self.path.startswith("/proxy/"):
            self._proxy("GET")
        else:
            super().do_GET()

    def do_POST(self):
        if self.path.startswith("/proxy/"):
            self._proxy("POST")
        else:
            self.send_error(405)

    def do_PUT(self):
        if self.path.startswith("/proxy/"):
            self._proxy("PUT")
        else:
            self.send_error(405)

    def do_DELETE(self):
        if self.path.startswith("/proxy/"):
            self._proxy("DELETE")
        else:
            self.send_error(405)

    # -- Core proxy logic -----------------------------------------------------

    def _proxy(self, method):
        target_url = self.path[len("/proxy/"):]
        if not proxy_target_allowed(target_url):
            self.send_error(403, "proxy target must be https:// to an allowed LLM API host (see DEV_PROXY_ALLOW)")
            return

        # Read request body.
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length) if content_length > 0 else None

        # Build upstream request.
        req = urllib.request.Request(target_url, data=body, method=method)
        for header in FORWARD_HEADERS:
            val = self.headers.get(header)
            if val:
                req.add_header(header, val)

        # Forward any provider-specific extra headers (X-* etc.).
        for key in self.headers:
            lower = key.lower()
            if lower.startswith("x-") and key not in FORWARD_HEADERS:
                req.add_header(key, self.headers[key])

        try:
            print(f"  PROXY {method} {target_url}")
            with urllib.request.urlopen(req, timeout=120) as resp:
                resp_body = resp.read()
                print(f"  PROXY {resp.status} ({len(resp_body)} bytes)")
                self.send_response(resp.status)
                self._copy_response_headers(resp)
                self._cors_headers()
                self.send_header("Content-Length", str(len(resp_body)))
                self.end_headers()
                self.wfile.write(resp_body)
        except urllib.error.HTTPError as e:
            err_body = e.read()
            print(f"  PROXY ERR {e.code} ({len(err_body)} bytes)")
            self.send_response(e.code)
            self.send_header("Content-Type", "application/json")
            self._cors_headers()
            self.send_header("Content-Length", str(len(err_body)))
            self.end_headers()
            self.wfile.write(err_body)
        except Exception as e:
            msg = str(e).encode()
            print(f"  PROXY EXC {e}")
            self.send_response(502)
            self.send_header("Content-Type", "text/plain")
            self._cors_headers()
            self.send_header("Content-Length", str(len(msg)))
            self.end_headers()
            self.wfile.write(msg)

    def _copy_response_headers(self, resp):
        for key, val in resp.getheaders():
            if key.lower() not in SKIP_RESPONSE_HEADERS:
                self.send_header(key, val)

    def _cors_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Max-Age", "86400")

    def end_headers(self):
        # Disable caching in dev so the browser always gets fresh files.
        self.send_header("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        super().end_headers()


if __name__ == "__main__":
    os.chdir(DIRECTORY)
    # ThreadingHTTPServer is required — single-threaded HTTPServer blocks on
    # HTTP/1.1 keep-alive connections, causing browsers to hang.
    server_cls = http.server.ThreadingHTTPServer
    with server_cls((BIND, PORT), ProxyHandler) as httpd:
        print(f"Serving on http://{BIND}:{PORT}/")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass
