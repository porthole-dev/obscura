#!/bin/sh
# Build the preview container and run a command in it with the repository at
# /src (default: the UI checks against libcamera's virtual camera).
#   LIBCAMERA_APKS=/path/to/x86_64/packages build-aux/preview/container/run.sh [command...]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
apks=${LIBCAMERA_APKS:?set LIBCAMERA_APKS to a directory with libcamera, libcamera-dev and libcamera-ipa x86_64 apks}
image=${IMAGE:-obscura-preview}
context=$(mktemp -d)
trap 'rm -rf "$context"' EXIT
cp "$here/Containerfile" "$here/virtual.yaml" "$context/"
mkdir "$context/apks"
for p in libcamera libcamera-dev libcamera-ipa; do
	cp "$(ls "$apks"/$p-[0-9]*.apk | tail -n1)" "$context/apks/"
done
podman build -q -t "$image" "$context" >/dev/null
[ $# -gt 0 ] || set -- build-aux/preview/check.sh --camera virtual
exec podman run --rm --security-opt label=disable \
	$( [ -e /dev/udmabuf ] && echo --device /dev/udmabuf ) \
	-v "$root:/src" -v obscura-preview-cargo:/cargo \
	-e PREVIEW_TARGET_DIR=/src/target/container "$image" "$@"
