#!/usr/bin/env bash
#
# Build the website as a gzipped static bundle and (optionally) push it
# to S3. Thin wrapper around `idealyst build --web --release --gzip` —
# the CLI does the staging and gzipping; this script only adds the S3
# upload step (which `aws s3 cp` makes annoying because each MIME type
# needs its own pass to get the right Content-Type alongside the
# shared `Content-Encoding: gzip`).
#
# Usage:
#   ./scripts/export-static.sh
#       # build + stage to target/idealyst/website/web/dist/
#
#   S3_BUCKET=my-bucket ./scripts/export-static.sh
#       # ...also upload
#
#   S3_BUCKET=my-bucket CF_DIST_ID=E123ABC ./scripts/export-static.sh
#       # ...also invalidate CloudFront
#
# S3 setup notes:
#   * Bucket needs index document = error document = index.html so SPA
#     routes (/install, /quickstart, ...) resolve client-side. For 200s
#     on deep links (not 404→fallback), front the bucket with
#     CloudFront and a viewer-request function that rewrites non-asset
#     URIs to /index.html.
#   * The site assumes it lives at the bucket root (`<base href="/">`
#     in index.html and absolute `/pkg/website.js` import). Subpath
#     hosting requires editing both.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
S3_BUCKET="${S3_BUCKET:-}"
CF_DIST_ID="${CF_DIST_ID:-}"

log() { printf '\033[1;36m[export]\033[0m %s\n' "$*"; }

log "release build + gzip (delegated to idealyst CLI)"
( cd "$HERE" && idealyst build --web --release --gzip )

# Default bundle location for `idealyst build --web --gzip` is
# `<project>/dist`.
DIST="$HERE/dist"

if [[ ! -d "$DIST" ]]; then
  echo "[export] expected bundle at $DIST but it does not exist" >&2
  echo "[export] (override via: idealyst build --web --release --gzip --out-dir <path>)" >&2
  exit 1
fi

log "bundle ready: $DIST ($(du -sh "$DIST" | cut -f1))"

if [[ -z "$S3_BUCKET" ]]; then
  cat <<EOF

To upload, re-run with S3_BUCKET set:
    S3_BUCKET=my-bucket-name $0

Or manually — one cp pass per MIME so Content-Type is correct while
Content-Encoding stays gzip:
    aws s3 cp $DIST s3://BUCKET/ --recursive \\
        --exclude '*' --include '*.wasm' \\
        --content-encoding gzip --content-type application/wasm \\
        --cache-control 'public, max-age=31536000, immutable' \\
        --metadata-directive REPLACE
    (repeat for js/html/ttf/json with their own --content-type)
EOF
  exit 0
fi

log "uploading to s3://$S3_BUCKET (Content-Encoding: gzip)"

upload_gzipped() {
  local ext="$1" ctype="$2" cache="$3"
  aws s3 cp "$DIST" "s3://$S3_BUCKET/" \
    --recursive \
    --exclude '*' --include "*.$ext" \
    --content-encoding gzip \
    --content-type "$ctype" \
    --cache-control "$cache" \
    --metadata-directive REPLACE
}

# Long cache for content-addressed-ish assets. wasm/js change on every
# release; the cheap defense without true cache-busting filenames is to
# tie cache lifetimes to the deploy cycle + invalidate CloudFront.
LONG_CACHE='public, max-age=31536000, immutable'
upload_gzipped wasm 'application/wasm'                          "$LONG_CACHE"
upload_gzipped js   'application/javascript; charset=utf-8'     "$LONG_CACHE"
upload_gzipped ttf  'font/ttf'                                  "$LONG_CACHE"
upload_gzipped json 'application/json'                          "$LONG_CACHE"

# Pre-compressed assets (the CLI's gzip step skipped these); ship as-is.
aws s3 cp "$DIST" "s3://$S3_BUCKET/" --recursive \
  --exclude '*' --include '*.woff' --include '*.woff2' \
  --content-type 'font/woff2' \
  --cache-control "$LONG_CACHE" \
  --metadata-directive REPLACE 2>/dev/null || true

# index.html must NOT be aggressively cached or users keep loading the
# old shell pointing at a stale wasm hash after a deploy.
aws s3 cp "$DIST/index.html" "s3://$S3_BUCKET/index.html" \
  --content-encoding gzip \
  --content-type 'text/html; charset=utf-8' \
  --cache-control 'public, max-age=0, must-revalidate' \
  --metadata-directive REPLACE

if [[ -n "$CF_DIST_ID" ]]; then
  log "invalidating CloudFront distribution $CF_DIST_ID"
  aws cloudfront create-invalidation \
    --distribution-id "$CF_DIST_ID" \
    --paths '/*' >/dev/null
fi

log "done"
