#!/bin/bash
# Count close timeouts over camera switches and full-resolution photos, with
# and without graphics offload.
P=build-aux/preview/preview.sh; log=${PREVIEW_TARGET_DIR:-target/preview}/run/app.log
for offload in on off; do
	if [ $offload = off ]; then export GDK_DISABLE=offload; fi
	OBSCURA_FAKE= $P start 360 720 >/dev/null; sleep 2
	for i in 1 2; do $P act switch-camera; sleep 2; done
	for i in 1 2; do $P act capture; sleep 3; done
	echo "offload $offload: $(grep -c close-timeout $log) close timeouts, $(grep -c 'camera-started' $log) opens, still-received $(grep -c still-received $log), photo-failed $(grep -c photo-failed $log), jpeg $(grep -c photo-jpeg-written $log)"
	grep -E "obscura-perf .* (camera-started|photo-failed)" $log | head -4
	$P stop
done
