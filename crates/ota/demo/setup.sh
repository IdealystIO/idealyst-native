#!/bin/sh
# Start a local MinIO (the S3 + CDN stand-in) and create the demo's bucket,
# readable by anyone, as a CDN would serve it. Safe to run again.
set -e

NAME=idealyst-ota-minio
if ! docker ps --format '{{.Names}}' | grep -qx "$NAME"; then
  if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    docker start "$NAME" >/dev/null
  else
    docker run -d --name "$NAME" -p 9000:9000 -p 9001:9001 \
      -e MINIO_ROOT_USER=otatest -e MINIO_ROOT_PASSWORD=otatest-secret \
      minio/minio server /data --console-address :9001 >/dev/null
  fi
fi
until curl -sf http://localhost:9000/minio/health/live >/dev/null; do sleep 1; done

. "$(dirname "$0")/minio.env"
aws s3api head-bucket --bucket ota-demo 2>/dev/null || aws s3api create-bucket --bucket ota-demo >/dev/null
aws s3api put-bucket-policy --bucket ota-demo --policy \
  '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["*"]},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::ota-demo/*"]}]}'
echo "MinIO is up: releases at http://localhost:9000/ota-demo/demo (console: http://localhost:9001, otatest / otatest-secret)"
