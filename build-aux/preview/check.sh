#!/bin/bash
# Drive the preview through Obscura's main flows and check each step in the
# OBSCURA_PERF log. Screenshots land in target/preview/run/shots (copied to
# target/preview/check/). Exit status is the number of failed checks.
#   build-aux/preview/check.sh [--no-build]
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$here/preview.sh
log=$root/target/preview/run/app.log
out=$root/target/preview/check
failed=0 step=0
[ "${1:-}" = --no-build ] || "$P" build >/dev/null || { echo "build failed" >&2; exit 100; }
rm -rf "$out" && mkdir -p "$out"

count() { grep -c "obscura-perf [0-9.]* $1" "$log" 2>/dev/null || true; }
# expect NAME EVENT [MINIMUM COUNT] [SECONDS]
expect() {
	local name=$1 event=$2 want=${3:-1} secs=${4:-10} i=0
	while [ "$(count "$event")" -lt "$want" ]; do
		sleep 0.2
		i=$((i + 1))
		if [ $i -ge $((secs * 5)) ]; then
			printf 'FAIL  %-34s no "%s" (x%s); last events:\n' "$name" "$event" "$want"
			grep "obscura-perf" "$log" | grep -v " viewfinder " | tail -4 | sed 's/^/        /'
			failed=$((failed + 1))
			return 1
		fi
	done
	printf 'ok    %-34s %s\n' "$name" "$(grep "obscura-perf [0-9.]* $event" "$log" | tail -1 | cut -d' ' -f2-)"
}
shot() { step=$((step + 1)); "$P" shot "$(printf %02d "$step")-$1" && cp "$root/target/preview/run/shots/$(printf %02d "$step")-$1.png" "$out/"; }
stop() { cp "$log" "$out/${1:-run}.log" 2>/dev/null; "$P" stop; }
start() { "$P" start "$@" >/dev/null || { echo "FAIL  start $*"; failed=$((failed + 1)); return 1; }; }

# Permission and camera states
for fake in denied nocamera busy; do
	OBSCURA_FAKE=$fake start 360 720 && expect "state: $fake" "status" 1 20 && shot "$fake"
	stop "$fake"
done

# The main flows, phone-sized
OBSCURA_FAKE=taimen start 360 720 || exit 100
expect "launch: first frame" "frame-first-presented" 1 30 && shot launch
"$P" act toggle-controls; expect "controls open" "controls open=true" && shot controls
"$P" act preview-set "ExposureValue=1.0"; expect "control reaches the camera" "fake-control ExposureValue"
"$P" act toggle-controls; expect "controls close" "controls open=false"

"$P" act capture; expect "full-res photo: reconfigure" "still-reconfigure" && expect "full-res photo: preview thumbnail" "thumbnail-preview" && expect "full-res photo: viewfinder back" "viewfinder-restored" && shot photo
"$P" act full-resolution
"$P" act capture; expect "fast photo: still" "still-received" 2
[ "$(count still-reconfigure)" -eq 1 ] && printf 'ok    %-34s\n' "fast photo: no reconfigure" || { printf 'FAIL  %-34s\n' "fast photo: no reconfigure"; failed=$((failed + 1)); }

"$P" point 180 250; sleep 0.3; "$P" click 1.2; expect "long press locks" "lock held=AE/AF" && shot locked
sleep 0.5
[ "$(count unlock)" -eq 0 ] && printf 'ok    %-34s\n' "lock survives the release" || { printf 'FAIL  %-34s\n' "lock survives the release"; failed=$((failed + 1)); }
"$P" point 120 320; sleep 0.3; "$P" click; expect "tap unlocks" "unlock" && expect "tap focuses" "tap " 1

"$P" act zoom; expect "zoom 2x" "zoom 2.0" && shot zoom
"$P" act zoom; "$P" act zoom; expect "zoom back to 1x" "zoom 1.0"

presented=$(count frame-first-presented)
"$P" act switch-camera; expect "switch camera" "frame-first-presented" $((presented + 1)) 20 && shot front
"$P" act mode video; expect "video mode" "frame-first-presented" $((presented + 2)) 20 && shot video
expect "encoders probed" "encoder-probed" 1 20
if grep -q "encoder-probed none" "$log"; then
	echo "skip  recording                          (no working video encoder here)"
else
	"$P" act capture; expect "record start" "recording-started" 1 15
	sleep 2; "$P" act capture; expect "record stop" "recording-saved" 1 20
fi
"$P" act mode photo; expect "photo mode" "frame-first-presented" $((presented + 3)) 20
"$P" act preferences; expect "preferences" "preferences" && shot preferences
stop main

# Wide window and landscape phone layouts
start 1000 680 && expect "wide: first frame" "frame-first-presented" 1 30 && "$P" act toggle-controls && sleep 1 && shot wide-sidebar
stop wide
start 760 360 && expect "landscape: first frame" "frame-first-presented" 1 30 && shot landscape
stop landscape

echo "$failed failed; screenshots in $out"
exit $failed
