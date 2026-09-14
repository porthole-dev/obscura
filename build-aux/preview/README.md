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

The fake camera exercises the interface, not libcamera. For the camera
worker itself (configuration, the viewfinder's dmabufs, the full-resolution
still session, closing and switching), run the checks against libcamera's
`virtual` pipeline in a container:

## With a real libcamera: the container

    LIBCAMERA_APKS=/path/to/x86_64/apks build-aux/preview/container/run.sh

`container/Containerfile` is Alpine edge with GTK, libadwaita, GStreamer,
mutter and a libcamera built with `-Dpipelines=auto,virtual` (put its
`libcamera`, `libcamera-dev` and `libcamera-ipa` x86_64 apks in
`LIBCAMERA_APKS`). `container/virtual.yaml` gives it two cameras shaped like
the Pixel 2 XL's (a 4:3 back camera with half-size and 16:9 modes, a 4:3
front one). `run.sh` builds the image, mounts the repository at /src, passes
/dev/udmabuf through for libcamera's buffers, and runs
`check.sh --camera virtual`, which builds natively and runs the main flows
with `OBSCURA_FAKE` empty (target/container/check/ has the logs and
screenshots). Any other command can follow `run.sh` instead.

`run.sh build-aux/preview/check.sh --camera pipewire` runs the same flows
through the PipeWire backend (`src/pipewire.rs`, what a sandboxed build
uses): a PipeWire daemon and WirePlumber start in the headless session, and
PipeWire's libcamera plugin publishes the virtual cameras as nodes. The app
connects to the PipeWire socket directly; the Camera portal leg
(`AccessCamera`, `OpenPipeWireRemote`) is not exercised there, since
xdg-desktop-portal needs a real session. Checks that need what PipeWire does
not forward (frame rates, full-resolution reconfiguration, metadata) are
skipped with a note.

The virtual pipeline cannot model everything: it has no raw stream,
controls or exposure metadata, and no autofocus or rotation, so the lock is
refused (and checked to be) and the taimen fake still covers those.
