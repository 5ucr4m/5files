#!/usr/bin/env sh
# Starts small throwaway servers for `cargo test --test remote_providers -- --ignored`.
#   SFTP   atmoz/sftp:alpine          127.0.0.1:22222   fivefiles / fivefilespass (+ ed25519 key)
#   FTP    delfer/alpine-ftp-server   127.0.0.1:22121   fivefiles / fivefilespass, PASV 22100-22109
#   S3     versity/versitygw (posix)  127.0.0.1:29000   fivefiles / fivefilessecret, bucket fivefiles-test
#   WebDAV hacdias/webdav             127.0.0.1:28080   fivefiles / fivefilespass
# The SFTP client key is written to $FIVEFILES_TEST_DIR (default: $TMPDIR/fivefiles-remote-tests),
# which is also where the tests look for it. Tear down with ./down.sh.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
TMP="${TMPDIR:-/tmp}"
DIR="${FIVEFILES_TEST_DIR:-${TMP%/}/fivefiles-remote-tests}"
mkdir -p "$DIR/keys"

if [ ! -f "$DIR/id_ed25519" ]; then
  ssh-keygen -q -t ed25519 -N "" -C fivefiles-tests -f "$DIR/id_ed25519"
fi
cp "$DIR/id_ed25519.pub" "$DIR/keys/id_ed25519.pub"

run() { name="$1"; shift; docker rm -f "$name" >/dev/null 2>&1 || true; docker run -d --name "$name" "$@" >/dev/null; echo "started $name"; }

run fivefiles-test-sftp -p 127.0.0.1:22222:22 \
  -v "$DIR/keys:/home/fivefiles/.ssh/keys:ro" \
  atmoz/sftp:alpine fivefiles:fivefilespass:1001::upload

run fivefiles-test-ftp -p 127.0.0.1:22121:21 -p 127.0.0.1:22100-22109:22100-22109 \
  -e USERS="fivefiles|fivefilespass|/ftp/fivefiles" -e ADDRESS=127.0.0.1 -e MIN_PORT=22100 -e MAX_PORT=22109 \
  --entrypoint /bin/sh delfer/alpine-ftp-server:latest \
  -c 'echo force_dot_files=YES >> /etc/vsftpd/vsftpd.conf && exec /sbin/tini -- /bin/start_vsftpd.sh'
# (vsftpd hides dotfiles from LIST by default; OpenDAL's FTP stat relies on LIST.)

run fivefiles-test-s3 -p 127.0.0.1:29000:7070 \
  -e ROOT_ACCESS_KEY=fivefiles -e ROOT_SECRET_KEY=fivefilessecret \
  --entrypoint sh versity/versitygw:latest \
  -c 'mkdir -p /gw/fivefiles-test && exec versitygw --port :7070 posix /gw'

run fivefiles-test-webdav -p 127.0.0.1:28080:6065 --tmpfs /data:uid=0,mode=1777 \
  -v "$HERE/webdav.yml:/etc/webdav/config.yml:ro" \
  hacdias/webdav:latest --config /etc/webdav/config.yml

# Wait until every port accepts connections.
for port in 22222 22121 29000 28080; do
  i=0
  until nc -z 127.0.0.1 "$port" 2>/dev/null; do
    i=$((i + 1)); [ "$i" -gt 60 ] && { echo "port $port did not come up" >&2; exit 1; }
    sleep 0.5
  done
done
# sshd needs a moment after the port opens to finish generating host keys.
sleep 2
echo "ready (key: $DIR/id_ed25519)"
