#!/bin/bash
# Run Obscura headless, with a fake camera, for screenshots and UI checks.
# See README.md next to this script.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
state=${PREVIEW_STATE:-$root/target/preview/run}
sysroot=${SYSROOT:-$HOME/.cache/obscura-sysroot}
cmd=${1:-help}
shift || true

native() { pkg-config --exists gtk4 libadwaita-1 libcamera 2>/dev/null; }
binary() {
	if native; then echo "$root/target/preview/release/obscura"; else echo "$root/target/preview/aarch64-unknown-linux-musl/release/obscura"; fi
}
load() { [ -f "$state/env" ] && . "$state/env"; }
call() { # object interface.method args...
	local path=$1 method=$2
	shift 2
	gdbus call --session --timeout 10 --dest "${DEST:-io.github.jertlok.Obscura}" --object-path "$path" --method "$method" "$@"
}
act() { # action [string parameter]
	if [ $# -gt 1 ]; then call /io/github/jertlok/Obscura org.gtk.Actions.Activate "$1" "[<'$2'>]" "{}" >/dev/null
	else call /io/github/jertlok/Obscura org.gtk.Actions.Activate "$1" "[]" "{}" >/dev/null; fi
}
input() { echo "$*" > "$state/input"; }

case $cmd in
build)
	if native; then
		CARGO_TARGET_DIR=$root/target/preview cargo build --release --features preview
	else
		CARGO_TARGET_DIR=$root/target/preview "$root/build-aux/cross-build.sh" --features preview
	fi
	;;
start) # [WIDTH HEIGHT]; OBSCURA_FAKE=taimen|denied|nocamera|busy (default taimen)
	width=${1:-360} height=${2:-720}
	"$0" stop 2>/dev/null || true
	rm -rf "$state" && mkdir -p "$state/shots" "$state/config/glib-2.0/settings" "$state/schemas"
	bin=$(binary)
	[ -x "$bin" ] || { echo "no preview build at $bin: run '$0 build'" >&2; exit 1; }
	run=()
	if ! native; then
		run=(qemu-aarch64-static -L "$sysroot")
		cp "$sysroot"/usr/share/glib-2.0/schemas/*.xml "$state/schemas/"
		# The sysroot has no fonts; lend it the host's.
		mkdir -p "$state/fonts"
		find /usr/share/fonts -name '*.[ot]tf' -path '*[Cc]antarell*' -exec ln -sf {} "$state/fonts/" \; -o -name '*.[ot]tf' -path '*adwaita*' -exec ln -sf {} "$state/fonts/" \;
		printf '<?xml version="1.0"?>\n<fontconfig><dir>%s</dir><cachedir>%s/fccache</cachedir></fontconfig>\n' "$state/fonts" "$state" > "$state/fonts.conf"
		export FONTCONFIG_FILE=$state/fonts.conf
	else
		cp /usr/share/glib-2.0/schemas/*.xml "$state/schemas/" 2>/dev/null || true
	fi
	cp "$root"/data/*.gschema.xml "$state/schemas/"
	glib-compile-schemas "$state/schemas"
	printf '[io/github/jertlok/Obscura]\nwindow-width=%s\nwindow-height=%s\n%b\n' "$width" "$height" "${PREVIEW_SETTINGS:-}" > "$state/config/glib-2.0/settings/keyfile"
	daemon=${DBUS_DAEMON:-$(command -v dbus-daemon || true)}
	[ -n "$daemon" ] || { echo "needs dbus-daemon (or DBUS_DAEMON=/path/to/it)" >&2; exit 1; }
	bus=${XDG_RUNTIME_DIR:-/tmp}/obscura-preview-bus-$$
	"$daemon" --config-file=/usr/share/dbus-1/session.conf --address="unix:path=$bus" --nofork >"$state/dbus.log" 2>&1 &
	echo "BUS_PID=$!" > "$state/env"
	for _ in $(seq 50); do [ -S "$bus" ] && break; sleep 0.1; done
	export DBUS_SESSION_BUS_ADDRESS=unix:path=$bus
	wl=obscura-preview-$$
	mutter --headless --wayland --no-x11 --virtual-monitor "${width}x${height}" --wayland-display "$wl" >"$state/mutter.log" 2>&1 &
	echo "MUTTER_PID=$!" >> "$state/env"
	for _ in $(seq 100); do [ -S "${XDG_RUNTIME_DIR:-/tmp}/$wl" ] && break; sleep 0.1; done
	env -u DISPLAY WAYLAND_DISPLAY="$wl" GDK_BACKEND=wayland GSK_RENDERER="${GSK_RENDERER:-cairo}" NO_AT_BRIDGE=1 \
		GSETTINGS_SCHEMA_DIR="$state/schemas" GSETTINGS_BACKEND=keyfile XDG_CONFIG_HOME="$state/config" HOME="$state" \
		OBSCURA_FAKE="${OBSCURA_FAKE-taimen}" OBSCURA_PERF=1 RUST_LOG="${RUST_LOG:-warn}" \
		"${run[@]}" "$bin" >"$state/app.log" 2>&1 &
	{ echo "APP_PID=$!"; echo "export DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS"; echo "WIDTH=$width HEIGHT=$height"; } >> "$state/env"
	for _ in $(seq 600); do grep -q "obscura-perf .* window-painted" "$state/app.log" 2>/dev/null && break; sleep 0.1; done
	grep -q "window-painted" "$state/app.log" || { echo "the app did not paint; see $state/app.log" >&2; exit 1; }
	# Pointer and keyboard through mutter's remote desktop interface.
	python3 "$here/input.py" "$state/input" >"$state/input.log" 2>&1 &
	echo "INPUT_PID=$!" >> "$state/env"
	for _ in $(seq 50); do grep -q ready "$state/input.log" 2>/dev/null && break; sleep 0.1; done
	echo "running: $state"
	;;
stop)
	load || exit 0
	kill "${INPUT_PID:-}" "${APP_PID:-}" "${MUTTER_PID:-}" "${BUS_PID:-}" 2>/dev/null || true
	rm -f "$state/env"
	;;
act) load; act "$@" ;;
shot) # NAME: window to $state/shots/NAME.png
	load
	rm -f "$state/shots/$1.png"
	act preview-shot "$state/shots/$1.png"
	for _ in $(seq 100); do [ -s "$state/shots/$1.png" ] && break; sleep 0.1; done
	[ -s "$state/shots/$1.png" ]
	;;
point) load; input point "$1" "$2" ;; # X Y in window coordinates (the window is at the origin)
click) load; input down; sleep "${1:-0.05}"; input up ;; # [SECONDS held]
key) load; input key "$1" ;; # KEYSYM, e.g. 0x20 for space
log) cat "$state/app.log" ;;
*)
	echo "usage: $0 build | start [W H] | stop | act ACTION [PARAM] | shot NAME | point X Y | click [SECONDS] | key KEYSYM | log" >&2
	exit 2
	;;
esac
