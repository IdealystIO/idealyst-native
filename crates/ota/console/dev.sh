#!/bin/sh
# Develop the console against the demo's local MinIO
# (crates/ota/demo/setup.sh). Extra arguments go to `idealyst dev`, e.g.
#   crates/ota/console/dev.sh --port 3001
set -e
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../demo/minio.env"
export OTA_CONSOLE_LOCATION="${OTA_CONSOLE_LOCATION:-s3://ota-demo/demo}"
cd "$here"
exec idealyst dev --web "$@"
