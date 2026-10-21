#!/usr/bin/env sh
# Removes the test containers and (unless KEEP_IMAGES=1) their images and the key dir.
set -u
docker rm -f atlas-test-sftp atlas-test-ftp atlas-test-s3 atlas-test-webdav >/dev/null 2>&1 || true
if [ "${KEEP_IMAGES:-0}" != "1" ]; then
  docker rmi atmoz/sftp:alpine delfer/alpine-ftp-server:latest versity/versitygw:latest hacdias/webdav:latest >/dev/null 2>&1 || true
fi
TMP="${TMPDIR:-/tmp}"
rm -rf "${ATLAS_TEST_DIR:-${TMP%/}/atlas-remote-tests}"
echo "down"
