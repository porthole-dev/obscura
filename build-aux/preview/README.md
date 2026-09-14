# Headless preview and UI checks

Run Obscura without a phone or a camera: in a headless mutter, on a private
D-Bus, with a fake camera shaped like the Pixel 2 XL's. Screenshots and an
automated walk through the main flows come out the other end.

    build-aux/preview/check.sh          # build, run every check, exit = failures
    build-aux/preview/preview.sh build  # or step by step:
    build-aux/preview/preview.sh start 360 720
    build-aux/preview/preview.sh act toggle-controls
    build-aux/preview/preview.sh shot controls   # target/preview/run/shots/controls.png
    build-aux/preview/preview.sh point 180 250 && build-aux/preview/preview.sh click 1.2
    build-aux/preview/preview.sh stop

## What it needs

- `mutter` (headless Wayland), `dbus-daemon` (or `DBUS_DAEMON=/path`),
  `glib-compile-schemas`, `gdbus`, and Python with PyGObject for input.
- Either a native build environment for Obscura (GTK 4, libadwaita,
  libcamera, GStreamer development files), or the aarch64 cross setup of
  `build-aux/cross-build.sh` plus `qemu-aarch64-static`: the preview binary
  then runs under qemu against the sysroot, with the host's fonts.

## How it works

`cargo build --features preview` adds `src/preview.rs`. Release builds never
enable the feature. It brings:

- **A fake camera**, chosen by `OBSCURA_FAKE`: `taimen` (default here; back
  and front cameras with the Pixel 2 XL's modes, controls, rotation and
  focus, including a full-resolution capture's round trip), `denied` (the
  portal refuses), `nocamera`, `busy`. With `OBSCURA_FAKE=` empty the real
  libcamera stack runs instead, for example its `virtual` pipeline handler
  on a native build (under qemu it cannot allocate dmabufs).
- **App actions** for the harness: `preview-shot` (write the window to a
  PNG after its next paint), `preview-tap` / `preview-hold` ("x,y"), and
  `preview-set` ("Control=value", through the controls panel).

Input is real: `input.py` holds a mutter remote-desktop session and turns
`point`/`click`/`key` into pointer and keyboard events. The app runs with
`OBSCURA_PERF=1`, and `check.sh` asserts on those log lines: the first
frame, controls reaching the camera, a full-resolution photo's reconfigure,
thumbnail and restored viewfinder, the long-press lock surviving its
release, unlock on tap, zoom, camera and mode switches, recording (skipped
without a working encoder), Preferences, and the wide and landscape
layouts. Logs and screenshots are kept in `target/preview/check/`.

The fake camera exercises the interface, not libcamera: the camera worker
itself (configuration, the still session, closing) still needs the
`virtual` pipeline on a native build, or a device.
