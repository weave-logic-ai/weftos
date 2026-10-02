#!/usr/bin/env bash
# Optional: real two-uid check of the machine mesh service (P3 package S).
#
# Runs the #[ignore] test that connects to a service owned by YOU from another
# account (default `nobody`) through `sudo -n -u`, and asserts that the kernel's
# peer credential identifies that account and that it cannot use admin verbs.
# Needs passwordless `sudo -u <user>`. Never part of `scripts/build.sh gate`;
# nothing here runs as root or touches /var, /etc, launchd or systemd.
#
# Usage: scripts/dev/mesh-two-uid.sh [user]
set -euo pipefail
cd "$(dirname "$0")/../.."
user="${1:-nobody}"
base="$(mktemp -d "${TMPDIR:-/var/tmp}/weftos-two-uid.XXXXXX")"
chmod 755 "$base"
trap 'rm -rf "$base"' EXIT
sudo -n -u "$user" true || { echo "passwordless 'sudo -u $user' is required" >&2; exit 1; }
WEFTOS_TWO_UID_USER="$user" WEFTOS_TWO_UID_DIR="$base" \
    cargo test -p clawft-mesh-service --test two_uid -- --ignored --nocapture
