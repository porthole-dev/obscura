# Known issues

Reported from real use, with what is known so far. Each says what was
observed, and what the evidence points at -- not a guess dressed up as a
diagnosis.

## Zoom is three different things, and only one of them is right

Observed: pinching in the viewfinder shows noisy, pixelated detail; the photo
that comes out is sharper AND framed slightly wider than the viewfinder
showed; a 4x video came out blurry.

All three are the same root cause. Zoom is implemented in three places that do
not agree:

* the **viewfinder** magnifies the preview stream in the widget
  (`viewfinder.rs`, `snapshot.scale()`), so it is enlarging pixels that were
  already downscaled -- that is the pixelation;
* a **photo** is cropped out of the full-resolution frame at save time
  (`photo.rs::crop`), which is sharp, but is not the same rectangle the
  viewfinder drew -- that is the framing mismatch;
* a **recording** ignores zoom completely: `app.rs` resets it to 1.0 when
  recording starts, with the comment "zoom crops photos only; recordings stay
  uncropped".

Fix: drive zoom through libcamera's `ScalerCrop` so the sensor/ISP crops once
and preview, stills and video all see the identical region. `camera.rs` already
reads `ScalerCropMaximum`. The viewfinder then stops scaling anything and
becomes WYSIWYG by construction, and video zoom exists for free.

## A 4K recording saved a zero-byte file

Observed: stopping a 4K recording showed a spinner for a long time, and the
file in the file manager was 0 bytes.

Not yet diagnosed. Two things worth checking first, in this order: whether the
muxer is ever finalised (a container that is never closed leaves nothing
readable), and whether the encoder was still draining when the file was
closed. Note that on this device SIGKILL to a venus user wedges the encoder
for the rest of the boot, so how the recording is torn down matters.

A spinner that never resolves also says the save is not reporting failure back
to the UI, which is its own bug: a failed save must say so.

## Hot pixels are not corrected

Observed: a black frame shows thousands of isolated coloured specks.

Measured 2026-09-16, two full-resolution dark frames back to back: 7625 and
7531 pixels above threshold, **5713 of them at identical coordinates**, where
random coincidence predicts 4.7. So this is a fixed defect pattern of roughly
5700 photosites (0.047% of 12.2 MP), not noise.

Nothing corrects it: libcamera's soft ISP has blc, awb, ccm, lsc, agc, af,
adjust and lux, and no defective-pixel stage. The module's OTP carries no
defect map either (AWB, LSC, AF and PDAF only). Android never showed these
because the Qualcomm ISP corrects them in hardware.

Fix: a defect-pixel correction stage comparing each pixel against its
same-colour neighbours and replacing wild outliers. Cheap in the debayer
shader, which already walks those neighbours. This belongs upstream rather
than here -- every device on the soft ISP has it.
