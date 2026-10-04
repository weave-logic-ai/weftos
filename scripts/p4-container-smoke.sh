#!/usr/bin/env bash
# Isolated engine smoke for ADR-103 P4. Requires an operator-built image with
# /usr/local/bin/weaver; does not contact or alter a user daemon or production container.
set -euo pipefail
ENGINE="${WEFTOS_SMOKE_ENGINE:-docker}"
IMAGE="${WEFTOS_SMOKE_IMAGE:?set WEFTOS_SMOKE_IMAGE to an operator-pinned image@sha256 digest containing /usr/local/bin/weaver}"
[[ "$ENGINE" == docker || "$ENGINE" == podman ]] || { echo 'Docker or Podman required' >&2; exit 2; }
[[ "$IMAGE" =~ @sha256:[0-9a-f]{64}$ ]] || { echo 'image must be pinned by digest' >&2; exit 2; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ID="$(date +%s)-$$"
DIR="$ROOT/target/p4-container-smoke/$ID"
PROJECT="$DIR/project"
RUNTIME="$DIR/runtime"
TRUST="$DIR/trust"
LINK="$DIR/link"
mkdir -p "$PROJECT" "$RUNTIME" "$TRUST" "$LINK"
NAME="weftos-p4-smoke-$ID"
CID=''
cleanup() {
  if [[ -n "$CID" ]]; then "$ENGINE" rm -f "$CID" >/dev/null 2>&1 || true; fi
  rm -rf "$DIR"
}
trap cleanup EXIT
EXTRA=()
if [[ "$ENGINE" == podman ]]; then EXTRA+=(--userns keep-id); fi
CID="$($ENGINE create --pull=never --name "$NAME" --label weftos.supervisor=project-smoke \
  --read-only --cap-drop ALL --security-opt no-new-privileges --pids-limit 64 \
  --network none --user "$(id -u):$(id -g)" --tmpfs /tmp:rw,nosuid,nodev,size=64m "${EXTRA[@]}" \
  --mount "type=bind,src=$PROJECT,dst=/weftos/project" \
  --mount "type=bind,src=$RUNTIME,dst=/weftos/run/smoke" \
  --mount "type=bind,src=$TRUST,dst=/weftos/trust,readonly" \
  --mount "type=bind,src=$LINK,dst=/weftos/parent,readonly" \
  --entrypoint /usr/local/bin/weaver "$IMAGE" kernel start --help)"
[[ "$CID" =~ ^[0-9a-f]{64}$ ]] || { echo "engine returned non-immutable ID: $CID" >&2; exit 1; }
[[ "$($ENGINE inspect --format '{{.Id}}' "$CID")" == "$CID" ]]
MOUNTS="$($ENGINE inspect --format '{{range .Mounts}}{{.Destination}}={{.RW}} {{end}}' "$CID")"
[[ "$MOUNTS" == *'/weftos/trust=false'* && "$MOUNTS" == *'/weftos/parent=false'* \
   && "$MOUNTS" == *'/weftos/run/smoke=true'* ]]
"$ENGINE" start -a "$CID" >"$ROOT/target/p4-container-smoke-$ID.log"
[[ "$($ENGINE inspect --format '{{.Id}}' "$CID")" == "$CID" ]]
[[ "$($ENGINE inspect --format '{{.State.ExitCode}}' "$CID")" == 0 ]]
HELP="$(cat "$ROOT/target/p4-container-smoke-$ID.log")"
[[ "$HELP" == *'--profile'* && "$HELP" == *'--project'* ]]
echo "PASS image/CLI preflight only: immutable ID $CID; NOT driver lifecycle acceptance"
