#!/bin/sh
# WeftOS machine mesh service uninstall. PRINTED by `weaver mesh uninstall-service`; nothing has been run.
# Run as root after reading. It stops and removes the unit, the root-owned binary and the runtime directory.
# It KEEPS the state directory, the journal, the log directory, mesh.toml and the account/group
# (account removal is listed below, commented out).
set -eu
[ "$(id -u)" -eq 0 ] || { echo 'run this script as root' >&2; exit 1; }

launchctl bootout system/ai.weftos.mesh || true
launchctl bootout system/ai.weftos.mesh-rundir || true
rm -f /Library/LaunchDaemons/ai.weftos.mesh.plist /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist /etc/newsyslog.d/weftos-mesh.conf
rm -f /usr/local/libexec/weftos/weaver
rmdir /usr/local/libexec/weftos 2>/dev/null || true
rm -rf /var/run/weftos

# /var/lib/weftos/mesh/node.key is KEPT (pass --purge-key to remove it). The state directory and journal are kept too.

# Account removal (commented out). A later account that reuses the uid would inherit its binds:
# run `weaver mesh bind revoke <uid>` for every bound uid first, and see docs/guides.
# dseditgroup -o edit -d USER -t user _weftos   (for each member)
# dscl . -delete /Users/_weftos
# dscl . -delete /Groups/_weftos
