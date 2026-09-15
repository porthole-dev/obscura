#!/bin/sh
# bump-aport.sh APKBUILD VERSION SHA512 -- point an aport at a new release.
#
# Sets pkgver to VERSION and the checksum of the "$pkgname-$pkgver.tar.gz"
# source to SHA512. pkgrel is 0 for a new version and one higher for the same
# version (a re-tagged release). Prints the commit subject's verb phrase, or
# nothing when the APKBUILD already matches.
set -eu
apkbuild=$1 version=$2 sum=$3

printf '%s\n' "$sum" | grep -qxE '[0-9a-f]{128}' || {
	echo "bump-aport.sh: not a sha512 checksum: $sum" >&2
	exit 1
}

value() { sed -n "s/^$1=//p" "$apkbuild" | head -n1; }
pkgname=$(value pkgname)
pkgver=$(value pkgver)
pkgrel=$(value pkgrel)
old="$pkgname-$pkgver.tar.gz"

[ "$(grep -cE "^[0-9a-f]{128}  $old\$" "$apkbuild")" = 1 ] || {
	echo "bump-aport.sh: $apkbuild has no single checksum line for $old" >&2
	exit 1
}

if [ "$pkgver" = "$version" ]; then
	grep -qxF "$sum  $old" "$apkbuild" && exit 0
	pkgrel=$((pkgrel + 1))
	subject="update the $version source archive"
else
	pkgrel=0
	subject="upgrade to $version"
fi
sed -i \
	-e "s/^pkgver=.*/pkgver=$version/" \
	-e "s/^pkgrel=.*/pkgrel=$pkgrel/" \
	-e "s/^[0-9a-f]\{128\}  $old\$/$sum  $pkgname-$version.tar.gz/" \
	"$apkbuild"
echo "$subject"
