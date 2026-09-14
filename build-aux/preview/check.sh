#!/bin/bash
# Drive the preview through Obscura's main flows and check each step in the
# OBSCURA_PERF log. Screenshots land in target/preview/run/shots (copied to
# target/preview/check/). Exit status is the number of failed checks.
#   build-aux/preview/check.sh [--no-build] [--camera fake|virtual]
# With --camera virtual the main flows run against the real libcamera stack
# (its virtual pipeline, see container/) instead of the fake camera.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$here/preview.sh
target=${PREVIEW_TARGET_DIR:-$root/target/preview}
log=$target/run/app.log
out=$target/check
failed=0 step=0
build=1 camera=fake
while [ $# -gt 0 ]; do
	case $1 in
	--no-build) build=0 ;;
	--camera) camera=$2; shift ;;
	esac
	shift
done
[ $build = 0 ] || "$P" build >/dev/null || { echo "build failed" >&2; exit 100; }
case $camera in
virtual) main_fake="" ;;
pipewire)
	main_fake=""
	export OBSCURA_BACKEND=pipewire PREVIEW_PIPEWIRE=1
	;;
*) main_fake=taimen ;;
esac
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
shot() { step=$((step + 1)); "$P" shot "$(printf %02d "$step")-$1" && cp "$target/run/shots/$(printf %02d "$step")-$1.png" "$out/"; }
# GTK and libadwaita criticals and warnings are bugs; a missing accessibility
# bus on a bare host is not the app's.
gtk_warnings() {
	grep -E "(Gtk|Gdk|Gsk|Adwaita|GLib|GLib-GObject|GLib-GIO)-(CRITICAL|WARNING)" "$log" 2>/dev/null | grep -v "Unable to register the application" || true
}
stop() {
	cp "$log" "$out/${1:-run}.log" 2>/dev/null
	local w
	w=$(gtk_warnings)
	if [ -n "$w" ]; then
		printf 'FAIL  %-34s\n%s\n' "no GTK warnings (${1:-run})" "$(echo "$w" | head -3 | sed 's/^/        /')"; failed=$((failed + 1))
	fi
	"$P" stop
}
# check NAME COMMAND...: run a tool if it is installed
tool() {
	local name=$1
	shift
	if ! command -v "$1" >/dev/null; then echo "skip  $name (no $1)"; return; fi
	if out_text=$("$@" 2>&1); then printf 'ok    %-34s\n' "$name"; else printf 'FAIL  %-34s\n%s\n' "$name" "$(echo "$out_text" | tail -5 | sed 's/^/        /')"; failed=$((failed + 1)); fi
}

# Data files
tool "metainfo validates" appstreamcli validate --no-net --pedantic "$root/data/io.github.jertlok.Obscura.metainfo.xml.in"
cp "$root/data/io.github.jertlok.Obscura.desktop.in" "$out/io.github.jertlok.Obscura.desktop"
tool "desktop file validates" desktop-file-validate "$out/io.github.jertlok.Obscura.desktop"
tool "settings schema compiles" glib-compile-schemas --strict --dry-run "$root/data"
for po in "$root"/po/*.po; do tool "translation $(basename "$po")" msgfmt -c -o /dev/null "$po"; done
start() { "$P" start "$@" >/dev/null || { echo "FAIL  start $*"; failed=$((failed + 1)); return 1; }; }

# Permission and camera states
for fake in denied nocamera busy; do
	OBSCURA_FAKE=$fake start 360 720 && expect "state: $fake" "status" 1 20 && shot "$fake"
	stop "$fake"
done

# The main flows, phone-sized
OBSCURA_FAKE=$main_fake start 360 720 || exit 100
expect "launch: first frame" "frame-first-presented" 1 30 && shot launch
"$P" act toggle-controls; expect "controls open" "controls open=true" && shot controls
if [ $camera = fake ]; then
	"$P" act preview-set "ExposureValue=1.0"; expect "control reaches the camera" "control ExposureValue"
fi
"$P" act toggle-controls; expect "controls close" "controls open=false"

if [ $camera = pipewire ]; then
	# One stream: every photo comes from it.
	"$P" act capture; expect "photo: preview thumbnail" "thumbnail-preview" && expect "photo: JPEG written" "photo-jpeg-written" 1 30 && shot photo
else
	"$P" act capture; expect "full-res photo: reconfigure" "still-reconfigure" && expect "full-res photo: preview thumbnail" "thumbnail-preview" && expect "full-res photo: viewfinder back" "viewfinder-restored" && shot photo
fi
if [ $camera = virtual ]; then expect "full-res photo: JPEG written" "photo-jpeg-written" 1 30; fi
"$P" act full-resolution
"$P" act capture; expect "fast photo: still" "still-received" 2
if [ $camera != pipewire ]; then
	[ "$(count still-reconfigure)" -eq 1 ] && printf 'ok    %-34s\n' "fast photo: no reconfigure" || { printf 'FAIL  %-34s\n' "fast photo: no reconfigure"; failed=$((failed + 1)); }
fi

# press NAME X Y SECONDS EVENT: a pointer press, tried three times. Under
# qemu an emulated app can miss one (a toast sliding in is enough); a real
# failure misses all three.
press() {
	local before
	before=$(count "$5")
	for _ in 1 2 3; do
		"$P" point "$2" "$3"; sleep 0.3; "$P" click "$4"
		for _ in $(seq 15); do [ "$(count "$5")" -gt "$before" ] && break 2; sleep 0.2; done
	done
	expect "$1" "$5" $((before + 1)) 1
}
press "long press is handled" 180 250 1.2 "lock" && shot locked
if grep -q "obscura-perf .* lock held" "$log"; then
	sleep 0.5
	[ "$(count unlock)" -eq 0 ] && printf 'ok    %-34s\n' "lock survives the release" || { printf 'FAIL  %-34s\n' "lock survives the release"; failed=$((failed + 1)); }
	press "tap unlocks" 120 320 0.05 "unlock"
elif [ $camera = fake ]; then
	printf 'FAIL  %-34s\n' "long press locks"; failed=$((failed + 1))
else
	echo "skip  lock                               (this camera reports nothing to hold)"
	press "tap reaches the viewfinder" 120 320 0.05 "tap "
fi

"$P" act zoom; expect "zoom 2x" "zoom 2.0" && shot zoom
"$P" act zoom; "$P" act zoom; expect "zoom back to 1x" "zoom 1.0"

presented=$(count frame-first-presented)
"$P" act switch-camera; expect "switch camera" "frame-first-presented" $((presented + 1)) 20 && shot front
"$P" act mode video; expect "video mode" "frame-first-presented" $((presented + 2)) 20 && shot video
if [ $camera = pipewire ]; then
	echo "skip  frame rate                         (PipeWire forwards no FrameDurationLimits)"
else
	"$P" act frame-rate; expect "frame rate chip cycles" "frame-rate Some" && expect "frame rate reaches the camera" "control FrameDurationLimits" && shot frame-rate
fi
expect "encoders probed" "encoder-probed" 1 20
if grep -q "encoder-probed none" "$log"; then
	echo "skip  recording                          (no working video encoder here)"
else
	"$P" act capture; expect "record start" "recording-started" 1 15
	sleep 2; "$P" act capture; expect "record stop" "recording-saved" 1 20
fi
"$P" act mode photo; expect "photo mode" "frame-first-presented" $((presented + 3)) 20
# Keyboard: Escape puts the controls away; Tab reaches the capture controls.
"$P" act toggle-controls; expect "controls open (keyboard)" "controls open=true" 2
sleep 0.5; "$P" key 0xff1b; expect "Escape closes controls" "controls open=false" 2
if python3 -c "import pyatspi" 2>/dev/null && "$P" a11y focus >/dev/null 2>&1; then
	reached=""
	for _ in $(seq 16); do "$P" key 0xff09; sleep 0.3; reached="$reached|$("$P" a11y focus)"; done
	for name in "Take Photo" "Switch Camera" "Camera Controls" "Main Menu"; do
		case $reached in *"$name"*) printf 'ok    %-34s\n' "Tab reaches $name" ;; *) printf 'FAIL  %-34s reached: %s\n' "Tab reaches $name" "$reached"; failed=$((failed + 1)) ;; esac
	done
	shot focus
	if names=$("$P" a11y names); then printf 'ok    %-34s\n' "every control has a name"; else printf 'FAIL  %-34s\n%s\n' "every control has a name" "$(echo "$names" | head -8 | sed 's/^/        /')"; failed=$((failed + 1)); fi
else
	echo "skip  keyboard focus and names (no AT-SPI here)"
fi
"$P" act preferences; expect "preferences" "preferences" && shot preferences
if grep -q "exceeds AdwApplicationWindow width" "$log"; then
	printf 'FAIL  %-34s %s\n' "fits a 360 px window" "$(grep -m1 -o 'requested [0-9]* px' "$log")"; failed=$((failed + 1))
else
	printf 'ok    %-34s\n' "fits a 360 px window"
fi
if grep -q "obscura-perf .* close-timeout" "$log"; then
	printf 'FAIL  %-34s %s\n' "cameras close promptly" "$(grep -c close-timeout "$log") close timeouts"; failed=$((failed + 1))
else
	printf 'ok    %-34s\n' "cameras close promptly"
fi
stop main

# Wide window and landscape phone layouts
# Italian, at phone width: nothing may overflow
PREVIEW_LANGUAGE=it OBSCURA_FAKE=taimen start 360 720 && expect "italian: first frame" "frame-first-presented" 1 30 && shot italian
"$P" act toggle-controls; sleep 1.5; shot italian-controls; "$P" act toggle-controls
"$P" act preferences; expect "italian: preferences" "preferences" && shot italian-preferences
if grep -q "exceeds AdwApplicationWindow width" "$log"; then printf 'FAIL  %-34s %s\n' "italian fits 360 px" "$(grep -m1 -o 'requested [0-9]* px' "$log")"; failed=$((failed + 1)); else printf 'ok    %-34s\n' "italian fits 360 px"; fi
stop italian

# Reduced motion and high contrast
PREVIEW_REDUCED_MOTION=1 ADW_DEBUG_HIGH_CONTRAST=1 OBSCURA_FAKE=taimen start 360 720 && expect "reduced motion: first frame" "frame-first-presented" 1 30
"$P" act capture; expect "reduced motion: photo" "thumbnail-preview"
"$P" act switch-camera; expect "reduced motion: switch" "frame-first-presented" 2 20 && shot high-contrast
stop reduced-motion

OBSCURA_FAKE=$main_fake start 1000 680 && expect "wide: first frame" "frame-first-presented" 1 30 && "$P" act toggle-controls && sleep 1 && shot wide-sidebar
stop wide
OBSCURA_FAKE=$main_fake start 760 360 && expect "landscape: first frame" "frame-first-presented" 1 30 && shot landscape
stop landscape

echo "$failed failed; screenshots in $out"
exit $failed
