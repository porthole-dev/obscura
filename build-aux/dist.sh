#!/bin/sh
# Write obscura-VERSION.tar.gz from HEAD into an aport directory and refresh
# its checksum. Until the repository is public the aport carries this tarball;
# after that its source= becomes the release tarball URL and this goes away.
#   build-aux/dist.sh <aport-dir>
set -eu
dir=${1:?usage: dist.sh <aport-dir>}
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
out="$dir/obscura-$version.tar.gz"
git diff --quiet HEAD || { echo "uncommitted changes; commit first" >&2; exit 1; }
git archive --format=tar --prefix="obscura-$version/" HEAD | gzip -n -9 > "$out"
sum=$(sha512sum "$out" | cut -d' ' -f1)
sed -i "/^sha512sums=/,\$d" "$dir/APKBUILD"
printf 'sha512sums="\n%s  obscura-%s.tar.gz\n"\n' "$sum" "$version" >> "$dir/APKBUILD"
echo "$out @ $(git rev-parse --short HEAD)"
