#!/usr/bin/env bash
# Download a pinned nats-server release into target/nats-server/<version>/
# and verify it against the release's SHA256SUMS.
#
# Usage: scripts/fetch-nats-server.sh [version]
set -euo pipefail

version="${1:-${NATS_SERVER_VERSION:-2.15.0}}"
version="${version#v}"

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=darwin ;;
    *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch=amd64 ;;
    aarch64 | arm64) arch=arm64 ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/target/nats-server/$version"
if [ -x "$dest/nats-server" ]; then
    echo "$dest/nats-server"
    exit 0
fi

name="nats-server-v$version-$os-$arch"
base="https://github.com/nats-io/nats-server/releases/download/v$version"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL --retry 3 -o "$tmp/$name.tar.gz" "$base/$name.tar.gz"
curl -fsSL --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
(cd "$tmp" && grep " $name.tar.gz\$" SHA256SUMS | sha256sum -c - >&2)

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dest"
mv "$tmp/$name/nats-server" "$dest/nats-server"
echo "$dest/nats-server"
