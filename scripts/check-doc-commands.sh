#!/usr/bin/env bash
# Doc-drift check: every `weaver ...` / `weft ...` command in the verified
# deployment docs must name subcommands and long flags that exist in the built
# binary (`--help`), and every key in a `[kernel.mesh]` TOML block must be a
# field of MeshConfig. Fails (exit 1) on any drift.
#
# Usage:
#   scripts/check-doc-commands.sh [--weaver PATH] [--weft PATH] [--list]
#   scripts/build.sh check-doc-commands
#
# Defaults: target/debug/weaver and target/debug/weft (scripts/build.sh native-debug).
# --list prints the extracted commands without checking them.
#
# Scope (the parts of the docs that were verified against the CLI):
#   docs/guides/agents.md                     whole file
#   docs/guides/weftos-deployment-sops.md     SOP 1 and SOP 3
# Extracted: fenced lines starting with `weaver`/`weft` (after `sudo`/`$`),
# `ExecStart=.../weaver ...` lines, and inline `weaver ...` code spans.
# Not checked: positional values, short flags, flag values, config keys other
# than [kernel.mesh]. A command is cut at ` > `, ` | `, ` && `, ` ; ` or ` # `.
# Keep prose that names a NON-existent command out of backticks that start
# with `weaver ` or `weft `, or it will be reported as drift.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

WEAVER="target/debug/weaver"
WEFT="target/debug/weft"
LIST_ONLY=false
while [ $# -gt 0 ]; do
    case "$1" in
        --weaver) WEAVER="${2:?--weaver needs a path}"; shift 2 ;;
        --weft) WEFT="${2:?--weft needs a path}"; shift 2 ;;
        --list) LIST_ONLY=true; shift ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

DOCS="docs/guides/agents.md:all docs/guides/weftos-deployment-sops.md:sop"
MESH_SRC="crates/clawft-types/src/config/kernel.rs"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/doc-commands.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

# ── extraction ───────────────────────────────────────────────────────
# Prints `file:line:command` for each command, `file:line:@key` for each
# [kernel.mesh] TOML key.
extract() { # file mode
    awk -v file="$1" -v mode="$2" '
    function clean(c) {
        sub(/^[ \t]+/, "", c)
        sub(/^\$ +/, "", c)
        sub(/^sudo +/, "", c)
        n = split(c, parts, / > | >> | \| | && | ; | # /)
        c = parts[1]
        sub(/[ \t\\]+$/, "", c)
        return c
    }
    function emit(c,   t) {
        t = clean(c)
        if (t ~ /^(weaver|weft)( |$)/ && t ~ / /) print file ":" start ":" t
    }
    BEGIN { on = (mode == "all") }
    mode == "sop" && /^## SOP 1/ { on = 1 }
    mode == "sop" && /^## SOP 2/ { on = 0 }
    mode == "sop" && /^## SOP 3/ { on = 1 }
    mode == "sop" && /^## SOP 4/ { on = 0 }
    !on { next }
    /^[ \t]*```/ {
        infence = !infence
        intoml = 0
        if (infence && $0 ~ /```toml/) intoml = 1
        mesh = 0
        next
    }
    infence {
        if (intoml) {
            if ($0 ~ /^\[/) mesh = ($0 ~ /^\[kernel\.mesh\]/)
            else if (mesh && $0 ~ /^[a-z_]+ *=/) {
                k = $0; sub(/ *=.*/, "", k); print file ":" NR ":@" k
            }
            next
        }
        line = $0; start = NR
        while (line ~ /\\[ \t]*$/ && (getline nxt) > 0) { sub(/\\[ \t]*$/, "", line); line = line " " nxt }
        if (line ~ /^[ \t]*ExecStart=/) {
            sub(/^[ \t]*ExecStart=/, "", line)
            sub(/^[^ ]*\//, "", line)
        }
        emit(line)
        next
    }
    {
        rest = $0; start = NR
        while (match(rest, /`[^`]+`/)) {
            span = substr(rest, RSTART + 1, RLENGTH - 2)
            rest = substr(rest, RSTART + RLENGTH)
            emit(span)
        }
    }' "$1"
}

: > "$WORK/all.txt"
for spec in $DOCS; do
    extract "${spec%%:*}" "${spec##*:}" >> "$WORK/all.txt"
done

if $LIST_ONLY; then
    sort -t: -k1,1 -k2,2n -u "$WORK/all.txt"
    exit 0
fi

# ── help parsing ─────────────────────────────────────────────────────
# help_of BIN sub... -> help text (cached)
help_of() {
    local bin="$1"; shift
    local key
    key="$WORK/help-$(basename "$bin")-$(printf '%s_' "$@" | tr -c 'A-Za-z0-9_\n' '_')"
    if [ ! -f "$key" ]; then
        "$bin" "$@" --help > "$key" 2>&1 || true
    fi
    cat "$key"
}
subcommands_of() { # bin sub... -> subcommand names
    help_of "$@" | awk '/^Commands:/ {on=1; next} on && /^$/ {exit} on && NF {print $1}'
}
flags_of() { # bin sub... -> long flags
    help_of "$@" | grep -oE -e '--[a-z0-9][a-z0-9-]*' | sort -u
}

fail=0
checked=0
report() { echo "DRIFT $1: $2"; fail=$((fail + 1)); }

check_command() { # loc command
    local loc="$1" cmd="$2" bin tok first
    local -a toks path flags
    read -r -a toks <<< "$cmd"
    first="${toks[0]}"
    case "$first" in
        weaver) bin="$WEAVER" ;;
        weft) bin="$WEFT" ;;
    esac
    path=(); flags=()
    local stopped=false t
    for t in "${toks[@]:1}"; do
        case "$t" in
            --*) flags+=("${t%%=*}") ;;
            -*) ;;
            *)
                $stopped && continue
                if subcommands_of "$bin" ${path[@]+"${path[@]}"} | grep -qx -- "$t"; then
                    path+=("$t")
                else
                    stopped=true
                    # a non-subcommand word right after a command that has
                    # subcommands is an unknown subcommand, not a positional
                    if [ -n "$(subcommands_of "$bin" ${path[@]+"${path[@]}"})" ] && ! [[ "$t" == \<* || "$t" == \"* || "$t" == \$* ]]; then
                        report "$loc" "\`$first ${path[*]+${path[*]}}\` has no subcommand \`$t\` (in: $cmd)"
                    fi
                fi
                ;;
        esac
    done
    # flags may be declared on the final command or any ancestor (globals)
    local f i ok
    for f in ${flags[@]+"${flags[@]}"}; do
        [ "$f" = "--help" ] && continue
        ok=false
        i=${#path[@]}
        while :; do
            if flags_of "$bin" ${path[@]+"${path[@]:0:$i}"} | grep -qx -- "$f"; then ok=true; break; fi
            [ "$i" -eq 0 ] && break
            i=$((i - 1))
        done
        $ok || report "$loc" "\`$first ${path[*]+${path[*]}}\` has no flag \`$f\` (in: $cmd)"
    done
    checked=$((checked + 1))
}

for b in "$WEAVER" "$WEFT"; do
    if [ ! -x "$b" ]; then
        echo "error: $b not found; build it with scripts/build.sh native-debug or pass --weaver/--weft" >&2
        exit 2
    fi
done

# MeshConfig field names and aliases
awk '/^pub struct MeshConfig/ {on=1; next} on && /^}/ {exit}
     on && /pub [a-z_]+:/ { s=$0; sub(/.*pub /, "", s); sub(/:.*/, "", s); print s }
     on && /alias = "/ { s=$0; sub(/.*alias = "/, "", s); sub(/".*/, "", s); print s }' "$MESH_SRC" | sort -u > "$WORK/mesh-keys.txt"
if [ ! -s "$WORK/mesh-keys.txt" ]; then
    echo "error: could not read MeshConfig fields from $MESH_SRC" >&2
    exit 2
fi

keys=0
while IFS= read -r row; do
    loc="${row%%:*}"; rest="${row#*:}"; line="${rest%%:*}"; cmd="${rest#*:}"
    if [ "${cmd:0:1}" = "@" ]; then
        key="${cmd:1}"
        keys=$((keys + 1))
        grep -qx -- "$key" "$WORK/mesh-keys.txt" || report "$loc:$line" "[kernel.mesh] key \`$key\` is not a MeshConfig field ($MESH_SRC)"
    else
        check_command "$loc:$line" "$cmd"
    fi
done < <(sort -t: -k1,1 -k2,2n -u "$WORK/all.txt")

echo "checked $checked commands and $keys [kernel.mesh] keys against $WEAVER and $WEFT: $fail drift finding(s)"
[ "$fail" -eq 0 ]
