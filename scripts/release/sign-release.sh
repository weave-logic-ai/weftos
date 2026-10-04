#!/usr/bin/env bash
# sign-release.sh <artifacts-dir> <tag>
#
# Writes <artifacts-dir>/weftos-release.json (the sha256 of every file in the
# directory, dist-manifest.json included, plus the signing time as
# "published") and weftos-release.json.sig (hex
# Ed25519 signature over "weftos-release-v1\n" + that file). `weaver update`
# refuses a release whose signature does not verify under the compiled-in
# WeaveLogic release key (crates/clawft-weave/src/commands/update_signature.rs).
#
# Key: WEAVELOGIC_RELEASE_KEY = the raw 32-byte Ed25519 seed as hex (the CI
# secret; the same key COG-008 signs cogs with). The script refuses to sign
# unless the key's public half equals the pinned WEFTOS_RELEASE_PUBKEY_HEX in
# crates/clawft-weave/src/commands/update_signature.rs. SIGN_RELEASE_EXPECT_PUBKEY replaces that
# expected value; it exists for tests with a throwaway key and changes nothing
# about what weaver trusts.
#
# Needs bash, OpenSSL 3 (Ed25519 -rawin), xxd, jq and sha256sum or shasum.
set -euo pipefail

die() { echo "sign-release: $*" >&2; exit 1; }

[[ $# -eq 2 ]] || die "usage: sign-release.sh <artifacts-dir> <tag>"
dir=$1
tag=$2
[[ -d $dir ]] || die "$dir is not a directory"
# Same tags release.yml triggers on (v[0-9]+.[0-9]+.[0-9]+*), limited to the
# characters weaver accepts in a tag.
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+[0-9A-Za-z.+_-]*$ ]] || die "tag $tag is not a vX.Y.Z release tag"
[[ ${WEAVELOGIC_RELEASE_KEY:-} =~ ^[0-9a-fA-F]{64}$ ]] \
    || die "WEAVELOGIC_RELEASE_KEY is unset or not a 64-hex-char Ed25519 seed; refusing to publish an unsigned release"
command -v openssl >/dev/null || die "openssl not found"
command -v jq >/dev/null || die "jq not found"
command -v xxd >/dev/null || die "xxd not found"

repo=$(cd "$(dirname "$0")/../.." && pwd)
pinned=$(sed -n 's/^pub const WEFTOS_RELEASE_PUBKEY_HEX: &str = "\([0-9a-f]\{64\}\)";$/\1/p' \
    "$repo/crates/clawft-weave/src/commands/update_signature.rs")
expect=${SIGN_RELEASE_EXPECT_PUBKEY:-$pinned}
[[ $expect =~ ^[0-9a-f]{64}$ ]] || die "cannot read the pinned public key"

manifest="$dir/dist-manifest.json"
[[ -f $manifest ]] || die "$manifest is missing; nothing to sign"
mtag=$(jq -r '.announcement_tag // empty' "$manifest")
[[ $mtag == "$tag" ]] || die "dist-manifest.json is for '$mtag', not $tag"

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
umask 077

# PKCS#8 DER for a raw Ed25519 seed. printf is a builtin, so the seed never
# appears in a process argument list.
printf '302e020100300506032b657004220420%s' "$WEAVELOGIC_RELEASE_KEY" | xxd -r -p > "$work/key.der"
unset WEAVELOGIC_RELEASE_KEY
openssl pkey -inform DER -in "$work/key.der" -pubout -outform DER -out "$work/pub.der"
pub=$(tail -c 32 "$work/pub.der" | xxd -p -c 64)
[[ $pub == "$expect" ]] || die "the signing key's public key $pub is not the pinned key $expect"

rm -f "$dir/weftos-release.json" "$dir/weftos-release.json.sig"
# The listing goes to a file first so a failing find or sort stops the script
# (set -e plus pipefail), which a process substitution would not.
files="$work/files"
find "$dir" -maxdepth 1 -type f -print0 | sort -z > "$files"
list="$work/list.tsv"
: > "$list"
while IFS= read -r -d '' f; do
    name=$(basename "$f")
    [[ $name =~ ^[0-9A-Za-z._+-]+$ ]] || die "asset name '$name' is not a plain file name"
    hash=$(sha256 "$f")
    [[ $hash =~ ^[0-9a-f]{64}$ ]] || die "could not hash $name (got '$hash')"
    printf '%s\t%s\n' "$name" "$hash" >> "$list"
done < "$files"
[[ -s $list ]] || die "no files to sign in $dir"

published=$(date -u +%Y-%m-%dT%H:%M:%SZ)
jq -R -s --arg tag "$tag" --arg published "$published" '
    split("\n") | map(select(length > 0) | split("\t"))
    | {schema: 1, kind: "weftos-release", tag: $tag, published: $published,
       assets: (map({key: .[0], value: .[1]}) | from_entries)}' "$list" > "$dir/weftos-release.json"

{ printf 'weftos-release-v1\n'; cat "$dir/weftos-release.json"; } > "$work/msg"
openssl pkeyutl -sign -inkey "$work/key.der" -keyform DER -rawin -in "$work/msg" -out "$work/sig"
openssl pkeyutl -verify -pubin -inkey "$work/pub.der" -keyform DER -rawin -in "$work/msg" -sigfile "$work/sig" >/dev/null \
    || die "the signature just made does not verify"
xxd -p -c 64 "$work/sig" | tr -d '\n' > "$dir/weftos-release.json.sig"
echo >> "$dir/weftos-release.json.sig"

count=$(jq '.assets | length' "$dir/weftos-release.json")
echo "sign-release: signed $count assets of $tag with key $pub"
