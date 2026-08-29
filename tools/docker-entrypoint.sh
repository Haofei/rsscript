#!/bin/sh
set -eu

# Named volumes replace the image-owned directories after the Dockerfile's
# build-time chown. Repair only the three cache mount roots on every start; do
# not recursively change the bind-mounted source tree.
mkdir -p \
    /work/target \
    "${CARGO_HOME:-/usr/local/cargo}/registry" \
    "${CARGO_HOME:-/usr/local/cargo}/git"
chown "${RSSCRIPT_UID:-1000}:${RSSCRIPT_GID:-1000}" \
    /work/target \
    "${CARGO_HOME:-/usr/local/cargo}/registry" \
    "${CARGO_HOME:-/usr/local/cargo}/git"

exec gosu "${RSSCRIPT_UID:-1000}:${RSSCRIPT_GID:-1000}" "$@"
