#!/usr/bin/env python3
"""Dev server for hello-web.

- SPA fallback: navigations (Accept: text/html) to unknown paths serve index.html.
  Asset requests for missing files get a real 404 (so the browser fails fast
  instead of trying to parse HTML as a JS module).
- gzip compression on the fly for text-ish + wasm responses, cached in memory.
  The release WASM is ~13MB uncompressed and compresses ~3-4x, which matters
  badly over slow tunnels (VS Code port forwarding etc.).
- Sensible caching: html is no-store (so reloads pick up the latest pkg/),
  hashed assets under pkg/ are cacheable for a short window.
"""
import gzip
import os
import sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent
INDEX = "index.html"

COMPRESSIBLE = {
    ".html", ".js", ".mjs", ".css", ".json", ".svg", ".txt", ".map", ".wasm",
}

# path-on-disk -> (mtime_ns, gzipped_bytes). Only populated for files we
# actually serve compressed. Single-process, so a plain dict is fine.
_GZIP_CACHE: dict[str, tuple[int, bytes]] = {}


def gzip_bytes(path: Path) -> bytes:
    key = str(path)
    mtime = path.stat().st_mtime_ns
    cached = _GZIP_CACHE.get(key)
    if cached and cached[0] == mtime:
        return cached[1]
    data = path.read_bytes()
    # mtime=0 so output is deterministic across restarts.
    out = gzip.compress(data, compresslevel=6, mtime=0)
    _GZIP_CACHE[key] = (mtime, out)
    return out


class SpaHandler(SimpleHTTPRequestHandler):
    # SimpleHTTPRequestHandler resolves MIME from this map; make sure .wasm is right.
    extensions_map = {
        **SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "application/javascript",
        ".mjs": "application/javascript",
    }

    def do_GET(self):
        path = Path(self.translate_path(self.path))
        missing = not path.exists() or (path.is_dir() and not (path / INDEX).exists())
        if missing and "text/html" in self.headers.get("Accept", ""):
            self.path = "/" + INDEX
            path = Path(self.translate_path(self.path))
            missing = False

        if missing:
            self.send_error(404, "File not found")
            return

        if path.is_dir():
            path = path / INDEX

        ext = path.suffix.lower()
        accept_enc = self.headers.get("Accept-Encoding", "")
        if ext in COMPRESSIBLE and "gzip" in accept_enc:
            body = gzip_bytes(path)
            ctype = self.guess_type(str(path))
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Vary", "Accept-Encoding")
            self._send_cache_headers(path)
            self.end_headers()
            self.wfile.write(body)
            return

        # Fall through to the stdlib path for non-compressible files / no-gzip clients.
        # We re-point self.path to the resolved file so the parent serves the right thing.
        rel = path.relative_to(ROOT).as_posix()
        self.path = "/" + rel
        super().do_GET()

    def _send_cache_headers(self, path: Path):
        # html should always re-fetch in dev; everything else can sit in the disk cache briefly.
        if path.suffix.lower() in {".html", ".htm"}:
            self.send_header("Cache-Control", "no-store")
        else:
            self.send_header("Cache-Control", "public, max-age=60")

    def end_headers(self):
        # Only inject default cache headers when not already handled by the gzip path.
        if "Cache-Control" not in self._headers_buffer_text():
            self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def _headers_buffer_text(self) -> str:
        # _headers_buffer is a list[bytes]; decode for a cheap substring check.
        return b"".join(getattr(self, "_headers_buffer", [])).decode("latin-1", "replace")


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    os.chdir(ROOT)
    handler = partial(SpaHandler, directory=str(ROOT))
    server = ThreadingHTTPServer(("0.0.0.0", port), handler)
    print(f"serving {ROOT} on http://localhost:{port} (SPA fallback + gzip)")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":
    main()
