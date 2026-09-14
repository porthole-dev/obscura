#!/bin/bash
# Regenerate data/screenshots/ (the metainfo's screenshots) from the preview
# with the fake camera.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
P=$here/preview.sh
shots=${PREVIEW_TARGET_DIR:-$root/target/preview}/run/shots
mkdir -p "$root/data/screenshots"
"$P" build >/dev/null
OBSCURA_FAKE=taimen "$P" start 400 800 >/dev/null
sleep 2
"$P" shot photo
"$P" act toggle-controls; sleep 1.5; "$P" shot controls
"$P" act toggle-controls; "$P" act mode video; sleep 2; "$P" shot video
"$P" stop
for name in photo controls video; do cp "$shots/$name.png" "$root/data/screenshots/$name.png"; done
ls -la "$root/data/screenshots"
