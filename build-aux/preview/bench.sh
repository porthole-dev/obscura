#!/bin/bash
# Timings from the preview: N launches, then N full-resolution photos, N fast
# photos and N camera switches in the last one. Logs go to
# $PREVIEW_TARGET_DIR/bench/, and bench-summary.py prints the tables.
#   build-aux/preview/bench.sh [N]   (OBSCURA_FAKE= for the real libcamera)
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
target=${PREVIEW_TARGET_DIR:-$(cd "$here/../.." && pwd)/target/preview}
P=$here/preview.sh log=$target/run/app.log n=${1:-3}
out=$target/bench
rm -rf "$out" && mkdir -p "$out"
count() { grep -c "obscura-perf [0-9.]* $1" "$log" 2>/dev/null || true; }
wait_count() { for _ in $(seq 150); do [ "$(count "$1")" -ge "$2" ] && return; sleep 0.2; done; echo "timeout: $1 x$2" >&2; }
for run in $(seq "$n"); do
	"$P" start 360 720 >/dev/null || exit 1
	wait_count frame-first-presented 1
	sleep 1
	if [ "$run" = "$n" ]; then
		for i in $(seq "$n"); do "$P" act capture; wait_count thumbnail-shown "$i"; sleep 1; done
		"$P" act full-resolution
		for i in $(seq "$n"); do "$P" act capture; wait_count thumbnail-shown $((n + i)); sleep 1; done
		for i in $(seq "$n"); do "$P" act switch-camera; wait_count frame-first-presented $((i + 1)); sleep 1; done
	fi
	cp "$log" "$out/run$run.log"
	"$P" stop
done
python3 "$here/bench-summary.py" "$out"/run*.log
