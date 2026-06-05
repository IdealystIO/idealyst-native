#!/usr/bin/env python3
"""Capture a screenshot from a running idealyst app via the Robot bridge.

Sends the `screenshot` verb over the bridge's newline-delimited JSON TCP
protocol, decodes the returned base64 PNG, writes it to disk, and (on
macOS) opens it.

Discovery order for the bridge port:
  1. --port N
  2. $IDEALYST_BRIDGE_PORT
  3. ~/.idealyst/apps/<app>-<pid>.json  (the file the bridge writes on bind)

Usage:
  python3 capture.py                      # auto-discover, default app filter
  python3 capture.py --app screenshot-demo --out shot.png
  python3 capture.py --port 9777
"""
import argparse
import base64
import glob
import json
import os
import socket
import struct
import subprocess
import sys


def discover_port(app_filter: str) -> int | None:
    env = os.environ.get("IDEALYST_BRIDGE_PORT")
    if env and env.isdigit() and int(env) != 0:
        return int(env)
    apps_dir = os.path.expanduser("~/.idealyst/apps")
    candidates = []
    for path in glob.glob(os.path.join(apps_dir, "*.json")):
        try:
            with open(path) as f:
                entry = json.load(f)
        except (OSError, json.JSONDecodeError):
            continue
        name = entry.get("name", "")
        port = entry.get("port")
        if not port:
            continue
        if app_filter and app_filter not in name and app_filter not in os.path.basename(path):
            continue
        candidates.append((os.path.getmtime(path), port, name))
    if not candidates:
        return None
    # Most recently registered app wins.
    candidates.sort(reverse=True)
    _, port, name = candidates[0]
    print(f"discovered app '{name}' on bridge port {port}")
    return port


def capture(port: int, width: int | None, height: int | None, source: str) -> tuple[bytes, int, int]:
    args: dict = {"source": source}
    if width:
        args["width"] = width
    if height:
        args["height"] = height
    req = json.dumps({"id": 1, "cmd": "screenshot", "args": args}) + "\n"

    s = socket.create_connection(("127.0.0.1", port), timeout=10)
    s.settimeout(30)
    s.sendall(req.encode())

    buf = b""
    while b"\n" not in buf:
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    line = buf.split(b"\n", 1)[0]
    if not line:
        raise RuntimeError("empty response from bridge (did the app crash?)")
    resp = json.loads(line)
    if "err" in resp:
        raise RuntimeError(f"bridge error: {resp['err']}")
    ok = resp["ok"]
    png = base64.b64decode(ok["png_base64"])

    sig = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
    if png[:8] != sig:
        raise RuntimeError(f"response is not a PNG (got {png[:8].hex()})")
    return png, ok.get("width", 0), ok.get("height", 0)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--app", default="screenshot-demo", help="substring of the app name/file to match")
    ap.add_argument("--port", type=int, default=None, help="bridge port (skips discovery)")
    ap.add_argument("--out", default="screenshot.png", help="output PNG path")
    ap.add_argument("--width", type=int, default=None, help="requested width (replay path only)")
    ap.add_argument("--height", type=int, default=None, help="requested height (replay path only)")
    ap.add_argument(
        "--source",
        choices=["auto", "client", "replay"],
        default="auto",
        help="auto (default): real client, fall back to wgpu replay; "
        "client: force the real native surface; replay: force the wgpu re-render",
    )
    ap.add_argument("--no-open", action="store_true", help="don't open the PNG after saving")
    args = ap.parse_args()

    port = args.port or discover_port(args.app)
    if not port:
        print("no bridge port found — is the app running under `idealyst dev`?", file=sys.stderr)
        return 2

    try:
        png, w, h = capture(port, args.width, args.height, args.source)
    except Exception as e:  # noqa: BLE001 - CLI: surface any failure clearly
        print(f"capture failed: {e}", file=sys.stderr)
        return 1

    with open(args.out, "wb") as f:
        f.write(png)
    # Cross-check the IHDR dimensions against the reported size.
    iw, ih = struct.unpack(">II", png[16:24]) if png[12:16] == b"IHDR" else (0, 0)
    print(f"saved {args.out}  reported={w}x{h}  ihdr={iw}x{ih}  bytes={len(png)}")

    if not args.no_open and sys.platform == "darwin":
        subprocess.run(["open", args.out], check=False)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
