#!/bin/bash
# Smoke-test the installed Flatpak builds in the headless preview session.
#   build-aux/flatpak/check.sh          the sandboxed build starts and shows a camera state
#   build-aux/flatpak/check.sh devel    the Devel build shows libcamera's virtual cameras
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$root/build-aux/preview/preview.sh
target=${PREVIEW_TARGET_DIR:-$root/target/preview}
log=$target/run/app.log
out=$root/target/flatpak/check
mkdir -p "$out"
failed=0
expect() { # NAME PATTERN SECONDS
	for _ in $(seq $(($3 * 5))); do grep -q "obscura-perf [0-9.]* $2" "$log" 2>/dev/null && { printf 'ok    %-30s %s\n' "$1" "$(grep -m1 "obscura-perf [0-9.]* $2" "$log" | cut -d' ' -f2-)"; return 0; }; sleep 0.2; done
	printf 'FAIL  %-30s no "%s"\n' "$1" "$2"; failed=$((failed + 1)); return 1
}
run() { flatpak run --user --no-documents-portal --env=OBSCURA_PERF=1 "$@"; }
if [ "${1:-}" = devel ]; then
	# Direct libcamera access, its virtual pipeline allowed for the test.
	export PREVIEW_COMMAND="flatpak run --user --no-documents-portal --env=OBSCURA_PERF=1 --env=LIBCAMERA_PIPELINES_MATCH_LIST=virtual --env=OBSCURA_SKIP_PORTAL=1 io.github.jertlok.Obscura//devel"
	name=devel
else
	export PREVIEW_COMMAND="flatpak run --user --no-documents-portal --env=OBSCURA_PERF=1 io.github.jertlok.Obscura//master"
	name=sandboxed
fi
PREVIEW_HOME=$HOME OBSCURA_FAKE= "$P" start 1000 700 >/dev/null || { echo "FAIL  $name did not start (see $log)"; exit 1; }
if [ $name = devel ]; then
	expect "libcamera backend: cameras" "camera-manager cameras=2" 30
	expect "first frame shown" "frame-first-presented" 30
	"$P" act switch-camera
	for _ in $(seq 100); do [ "$(grep -c "obscura-perf [0-9.]* session-opened" "$log")" -ge 2 ] && break; sleep 0.2; done
	expect "camera switch" "session-opened" 1 && [ "$(grep -c "obscura-perf [0-9.]* session-opened" "$log")" -ge 2 ] || { echo "FAIL  camera switch opened nothing new"; failed=$((failed + 1)); }
else
	# No portal in the headless session: the sandboxed build must say so
	# rather than hang or crash.
	expect "portal answered" "portal" 30
	expect "shows a camera state" "status" 30
fi
# ponytail: no screenshot: the preview-shot hook is not in release builds.
cp "$log" "$out/$name.log"
"$P" stop
echo "$failed failed; $out"
exit $failed
