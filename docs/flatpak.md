# Flatpak and Flathub: design note

Status: design only. Nothing here is implemented yet.

Obscura today opens cameras with libcamera directly, which needs the
`/dev/media*` and `/dev/video*` nodes. A Flathub build cannot have them: the
sandboxed way to a camera is the Camera portal, which hands out a PipeWire
remote. This note covers what such a build needs, what that path can and
cannot do, and how the backend should grow to support it.

## Two builds

| | Flathub | Development flatpak |
|---|---|---|
| Camera access | Camera portal → PipeWire | `--device=all` → libcamera |
| libcamera | not needed in the app | bundled as a module |
| Controls | the subset PipeWire forwards (below) | everything, as today |
| RAW / DNG | no | yes |
| Accepted on Flathub | yes | no (`--device=all`) |

The development build is a convenience for testing a sandboxed runtime on a
real device; the Flathub build is the one to design for.

## Manifest outline

```yaml
id: io.github.jertlok.Obscura
runtime: org.gnome.Platform
runtime-version: "49"
sdk: org.gnome.Sdk
sdk-extensions: [org.freedesktop.Sdk.Extension.rust-stable]
command: obscura
finish-args:
  - --socket=wayland
  - --socket=fallback-x11
  - --share=ipc
  - --device=dri                      # GPU for GTK; dmabuf import
  - --filesystem=xdg-pictures/Obscura:create
  - --filesystem=xdg-videos/Obscura:create
  - --socket=pulseaudio               # the microphone for recordings
  - --talk-name=org.sigxcpu.Feedback  # shutter sound / vibration (feedbackd)
  - --system-talk-name=net.hadess.SensorProxy   # which way up the device is
build-options:
  append-path: /usr/lib/sdk/rust-stable/bin
  env: { CARGO_HOME: /run/build/obscura/cargo }
modules:
  - name: obscura
    buildsystem: meson
    sources:
      - type: dir
        path: .
      - cargo-sources.json            # flatpak-cargo-generator over Cargo.lock
```

The GNOME 49 platform already has GTK 4.20+, libadwaita 1.8+, GStreamer with
the good plugins (jpegenc, videoflip, videocrop, x264 is *not* there: the
encoder probe falls back to openh264enc or a VA-API encoder through the
GStreamer VA plugin), and PipeWire's client library. The development build
adds one module:

```yaml
  - name: libcamera
    buildsystem: meson
    config-opts: [-Dpipelines=auto, -Dipas=all, -Dcam=disabled, -Dqcam=disabled,
                  -Dgstreamer=disabled, -Dv4l2=false, -Dpycamera=disabled, -Ddocumentation=disabled]
    sources: [{ type: git, url: https://git.libcamera.org/libcamera/libcamera.git, tag: v0.7.2 }]
```

(its IPA modules need signing keys generated at build time, which libcamera's
meson does when `openssl` is in the SDK).

## The portal path

1. `org.freedesktop.portal.Camera.AccessCamera` — Obscura already does this
   (`src/portal.rs`), so the permission dialog and Settings › Privacy work the
   same.
2. `OpenPipeWireRemote` returns a file descriptor; `pw_context_connect_fd`
   connects to it. The remote only shows the camera nodes.
3. Each camera is a PipeWire node (`media.class = Video/Source`,
   `media.role = Camera`) created by PipeWire's libcamera SPA plugin, with
   `api.libcamera.location` (front/back) and `api.libcamera.rotation`.
4. Obscura connects a stream to the node and negotiates a format from the
   node's `EnumFormat` params (sizes, frame rates, NV12/RGB/…), asking for
   `SPA_DATA_DmaBuf` so frames still reach GTK as dmabufs.
5. Buffers carry `SPA_META_VideoTransform`, which PipeWire derives from the
   configuration's orientation — the same value Obscura uses today, so the
   rotation logic carries over unchanged.

## What survives through PipeWire

From PipeWire 1.6's `spa/plugins/libcamera/libcamera-source.cpp`:

- **Controls.** The node publishes the camera's `ControlInfoMap` as
  `PropInfo`: Brightness, Contrast, Saturation, Sharpness, ExposureTime and
  AnalogueGain as the standard `SPA_PROP_*`, and every other control as
  `SPA_PROP_START_CUSTOM + control id` — but only controls of type **Bool,
  Int32 or Float** with a single value. Setting one is
  `pw_node_set_param(SPA_PARAM_Props)`.
  - Survive: AeEnable, ExposureTimeMode, ExposureTime, AnalogueGainMode,
    AnalogueGain, ExposureValue, AeExposureMode, AeConstraintMode, AwbEnable,
    AwbMode, Brightness, Contrast, Saturation, Sharpness, Gamma, AfMode,
    AfTrigger, LensPosition.
  - Lost: FrameDurationLimits (Int64 array: frame rate follows the
    negotiated format instead), ColourGains (Float array), AfWindows
    (Rectangle array: tap focuses the default area), ScalerCrop (digital zoom
    stays a crop in the app).
  - Controls act on the node, so they are shared with any other client of
    the same camera.
- **Metadata: none.** Per-frame libcamera metadata is not forwarded. The
  capture-info chips (ISO, shutter, colour temperature), automatic values in
  the controls, AfState (the focus ring's yellow/red) and the AE/AF lock
  (which holds the exposure the camera chose) all have nothing to read.
  They must degrade, not break: chips and the lock hide, the focus ring times
  out as it does on a camera without AfState. Upstream PipeWire would need a
  `SPA_META` or a props event for this; worth proposing.
- **Streams: one.** The node configures a single `VideoRecording` role
  stream. **No raw stream, so no DNG**, and no separate viewfinder and still
  streams.
- **Full-resolution photos** would renegotiate the stream to the photo size
  and back (the plugin reconfigures the camera on a format change), which is
  slower than today's in-process reconfigure. The Flathub build should
  default to photos from the viewfinder stream at the largest size that
  still gives a fluid preview, with full resolution as the opt-in it already
  is.

## Backend plan

The UI already talks to the camera only through `camera::Cmd` and
`camera::Event` (see `src/camera.rs`), and the preview's fake camera proves a
second implementation fits. A `PipeWireBackend` would:

- live in `src/pipewire.rs`, on its own thread with a `pw::main_loop`, using
  the `pipewire` crate (pipewire-rs), and expose the same `send(Cmd)`, event
  callback and `FrameSlot`;
- `Event::Cameras`: from the registry's camera nodes, with facing from
  `api.libcamera.location` and the model from `node.description`;
- `Cmd::Open`: connect a stream; `Session.modes` and `fps` from `EnumFormat`,
  `controls` from `PropInfo` (min/max/default/enum labels map directly onto
  `ControlDesc`), `raw: false`, `af_windows: false`, rotation from the
  transform meta;
- frames: dequeue a buffer, wrap its dmabuf plane in `Frame` (the dmabuf path
  in `viewfinder.rs` is unchanged; `ret` requeues the pw buffer instead of a
  libcamera request), deliver through the `FrameSlot`;
- `Cmd::SetControl`: `SPA_PARAM_Props` with the mapped prop id;
- `Cmd::Capture`: copy the next buffer into a `Still` with empty metadata;
  `Cmd::FullResolution` triggers a format renegotiation round trip;
- `Cmd::Record`: unchanged — frames go to the same GStreamer recorder.

Selection in `app.rs`: `ashpd::is_sandboxed()` and a granted portal pick the
PipeWire backend, otherwise the libcamera one. The UI needs one addition: a
`Session` capability for "has metadata", so the chips, the lock and automatic
readouts hide instead of showing stale defaults.

## Testing without a device

The container in `build-aux/preview/container` already has libcamera's
virtual pipeline; adding `pipewire`, `wireplumber` and PipeWire's libcamera
plugin gives a real PipeWire camera node to run `check.sh` against, with a
`--camera pipewire` mode that asserts the degraded behaviour (no chips, no
lock, no RAW) as well as the flows that must still work.
