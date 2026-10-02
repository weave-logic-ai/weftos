#!/bin/sh
# WeftOS machine mesh service install (launchd). PRINTED by `weaver mesh install-service`;
# nothing has been run. Read it, then run it as an administrator (sudo sh install.sh).
#
# What it does:
#  - creates the _weftos account and the _weftos group (no login, never root)
#  - adds YOU (the invoking user) to the _weftos group: /var/run/weftos is owned by that group with
#    mode 0750, so only group members can reach the mesh socket. Log out and in (or start a new
#    login session) for the membership to apply.
#  - installs a root-owned copy of weaver at /usr/local/libexec/weftos/weaver (the service never runs from a
#    user-writable path)
#  - writes /etc/weftos/mesh.toml (only if absent) and installs the unit; it does NOT start the service
set -eu
[ "$(id -u)" -eq 0 ] || { echo 'run this script as root (sudo sh install.sh)' >&2; exit 1; }
TARGET_USER="${SUDO_USER:-}"
[ -n "$TARGET_USER" ] && [ "$TARGET_USER" != root ] || { echo 'run it with sudo from your own account so the group member can be your user' >&2; exit 1; }

# --- account and group ---
if ! dscl . -read /Groups/_weftos >/dev/null 2>&1; then
  ID=300
  while dscl . -list /Groups PrimaryGroupID | awk '{print $2}' | grep -qx "$ID" ||
        dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$ID"; do
    ID=$((ID + 1))
  done
  dscl . -create /Groups/_weftos
  dscl . -create /Groups/_weftos PrimaryGroupID "$ID"
  dscl . -create /Groups/_weftos RealName "WeftOS mesh service"
  dscl . -create /Groups/_weftos Password '*'
fi
if ! dscl . -read /Users/_weftos >/dev/null 2>&1; then
  GID_=$(dscl . -read /Groups/_weftos PrimaryGroupID | awk '{print $2}')
  dscl . -create /Users/_weftos
  dscl . -create /Users/_weftos UniqueID "$GID_"
  dscl . -create /Users/_weftos PrimaryGroupID "$GID_"
  dscl . -create /Users/_weftos RealName "WeftOS mesh service"
  dscl . -create /Users/_weftos UserShell /usr/bin/false
  dscl . -create /Users/_weftos NFSHomeDirectory /var/empty
  dscl . -create /Users/_weftos Password '*'
  dscl . -create /Users/_weftos IsHidden 1
fi
# add _weftos membership for the invoking user
dseditgroup -o edit -a "$TARGET_USER" -t user _weftos

# --- directories and modes ---
install -d -m 0755 -o root -g wheel /etc/weftos /var/lib/weftos /usr/local/libexec/weftos
install -d -m 0700 -o _weftos -g _weftos /var/lib/weftos/mesh
install -d -m 0750 -o _weftos -g _weftos /var/run/weftos
install -d -m 0750 -o _weftos -g _weftos /var/log/weftos

# --- binary (root-owned copy) ---
install -m 0755 -o root -g wheel '/opt/we ftos/it'\''s/weaver' /usr/local/libexec/weftos/weaver

# --- adopt the existing node key (the key will exist in two places; node id unchanged) ---
[ ! -e /var/lib/weftos/mesh/node.key ] || { echo '/var/lib/weftos/mesh/node.key already exists; not overwriting' >&2; exit 1; }
install -m 0600 -o _weftos -g _weftos '/Users/a b/.weftos/run/node.key' /var/lib/weftos/mesh/node.key

# --- configuration (kept if already present) ---
if [ ! -e /etc/weftos/mesh.toml ]; then
cat > /etc/weftos/mesh.toml.new <<'WEFTOS_EOF'
# WeftOS machine mesh service (written by `weaver mesh install-service`).
state_dir = "/var/lib/weftos/mesh"
socket = "/var/run/weftos/mesh.sock"
listen = "127.0.0.1:9489"
health_listen = "127.0.0.1:9490"
admin_uids = [501]
WEFTOS_EOF
chown root:wheel /etc/weftos/mesh.toml.new && chmod 0644 /etc/weftos/mesh.toml.new && mv /etc/weftos/mesh.toml.new /etc/weftos/mesh.toml
fi

# --- unit ---
cat > /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist.new <<'WEFTOS_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ai.weftos.mesh-rundir</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/sh</string>
        <string>-c</string>
        <string>/bin/mkdir -p /var/run/weftos &amp;&amp; /usr/sbin/chown _weftos:_weftos /var/run/weftos &amp;&amp; /bin/chmod 0750 /var/run/weftos</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
WEFTOS_EOF
chown root:wheel /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist.new && chmod 0644 /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist.new && mv /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist.new /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist
cat > /Library/LaunchDaemons/ai.weftos.mesh.plist.new <<'WEFTOS_EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ai.weftos.mesh</string>
    <key>UserName</key>
    <string>_weftos</string>
    <key>GroupName</key>
    <string>_weftos</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/libexec/weftos/weaver</string>
        <string>mesh</string>
        <string>serve</string>
        <string>--config</string>
        <string>/etc/weftos/mesh.toml</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/var/lib/weftos/mesh</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>WEFTOS_MESH_STATE_DIR</key>
        <string>/var/lib/weftos/mesh</string>
        <key>WEFTOS_MESH_SOCKET</key>
        <string>/var/run/weftos/mesh.sock</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>/var/log/weftos/mesh.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/weftos/mesh.log</string>
</dict>
</plist>
WEFTOS_EOF
chown root:wheel /Library/LaunchDaemons/ai.weftos.mesh.plist.new && chmod 0644 /Library/LaunchDaemons/ai.weftos.mesh.plist.new && mv /Library/LaunchDaemons/ai.weftos.mesh.plist.new /Library/LaunchDaemons/ai.weftos.mesh.plist
# macOS clears /var/run at boot; ai.weftos.mesh-rundir recreates /var/run/weftos (group _weftos, 0750).
launchctl bootstrap system /Library/LaunchDaemons/ai.weftos.mesh-rundir.plist || true

# --- done; NOT started ---
echo 'installed. _weftos membership for '"$TARGET_USER"' takes effect at next login.'
echo 'stop any collapsed user daemon first (it holds 9489): weaver kernel stop'
echo 'then enable the service with:'
echo '  sudo launchctl bootstrap system /Library/LaunchDaemons/ai.weftos.mesh.plist'
echo 'afterwards: weaver mesh status, compare the machine key fingerprint out of band, weaver mesh trust'
# ENABLE (exact command): launchctl bootstrap system /Library/LaunchDaemons/ai.weftos.mesh.plist
