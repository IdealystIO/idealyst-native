#!/bin/sh
# Build the demo's bundle (the `Panel` in src/lib.rs) and publish it to the
# local MinIO. The running window picks it up at its next check.
set -e
. "$(dirname "$0")/minio.env"
exec idealyst ota publish "$(dirname "$0")" "$@"
