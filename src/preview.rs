// SPDX-License-Identifier: GPL-3.0-or-later
//! Development only, behind the `preview` cargo feature: a fake camera and
//! D-Bus hooks for build-aux/preview, so the interface can be driven and
//! photographed on a machine without the device. Release builds never
//! enable it.
//!
//! OBSCURA_FAKE selects the fake: `taimen` (a Pixel 2 XL-shaped back and
//! front camera: its modes, controls, rotation 270/90 and focus), `denied`
//! (the portal says no), `nocamera`, or `busy`. Unset, the real libcamera
//! stack runs, e.g. with its `virtual` pipeline on a native build.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use relm4::ComponentSender;
use relm4::adw::prelude::*;
use relm4::gtk::{self, gio, glib, gsk};

use crate::app::{App, Msg};
use crate::camera::{CameraInfo, Cmd, ControlDesc, Event, Facing, Frame, FrameSlot, Internal, Kind, LibcameraBackend, Metadata, Mode, Session, Still};
use crate::portal::Access;

fn fake() -> Option<String> {
    std::env::var("OBSCURA_FAKE").ok().filter(|f| !f.is_empty())
}

pub fn access() -> Option<Access> {
    Some(match fake()?.as_str() {
        "denied" => Access::Denied,
        _ => Access::Unavailable("OBSCURA_FAKE".into()),
    })
}

/// App actions for the harness: `preview-shot` (a PNG path), `preview-tap`
/// and `preview-hold` ("x,y" in window coordinates), `preview-set`
/// ("Control=value", through the controls panel).
pub fn install(window: &relm4::adw::ApplicationWindow, sender: &ComponentSender<App>) {
    let app = relm4::main_application();
    let string = Some(glib::VariantTy::STRING);

    let shot = gio::SimpleAction::new("preview-shot", string);
    let win = window.clone();
    shot.connect_activate(move |_, v| {
        if let Some(path) = v.and_then(|v| v.get::<String>()) {
            screenshot(&win, path);
        }
    });
    app.add_action(&shot);

    for (name, hold) in [("preview-tap", false), ("preview-hold", true)] {
        let action = gio::SimpleAction::new(name, string);
        let s = sender.clone();
        action.connect_activate(move |_, v| {
            let point = v.and_then(|v| v.get::<String>()).and_then(|p| {
                let (x, y) = p.split_once(',')?;
                Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
            });
            if let Some((x, y)) = point {
                s.input(if hold { Msg::Lock(x, y) } else { Msg::TapFocus(x, y) });
            }
        });
        app.add_action(&action);
    }

    let set = gio::SimpleAction::new("preview-set", string);
    let s = sender.clone();
    set.connect_activate(move |_, v| {
        if let Some((name, value)) = v.and_then(|v| v.get::<String>()).and_then(|p| {
            let (n, v) = p.split_once('=')?;
            Some((n.to_string(), v.parse::<f64>().ok()?))
        }) {
            s.input(Msg::PreviewSet(name, value));
        }
    });
    app.add_action(&set);
}

/// Write the window, dialogs included, to `path` after its next paint.
fn screenshot(window: &relm4::adw::ApplicationWindow, path: String) {
    let Some(child) = window.child() else { return };
    child.queue_draw();
    let Some(clock) = child.frame_clock() else { return };
    let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
    let (slot, clock2, win) = (handler.clone(), clock.clone(), window.clone());
    *handler.borrow_mut() = Some(clock.connect_after_paint(move |_| {
        let snapshot = gtk::Snapshot::new();
        win.snapshot_child(&child, &snapshot);
        let renderer = gsk::CairoRenderer::new();
        if renderer.realize_for_display(&WidgetExt::display(&win)).is_ok() {
            let saved = snapshot.to_node().map(|n| renderer.render_texture(n, None).save_to_png(&path));
            renderer.unrealize();
            perf!("preview-shot", "{path} ok={}", matches!(saved, Some(Ok(()))));
        }
        if let Some(id) = slot.borrow_mut().take() {
            clock2.disconnect(id);
        }
    }));
}

fn enums(names: &[&str]) -> Vec<(i32, String)> {
    names.iter().enumerate().map(|(i, n)| (i as i32, n.to_string())).collect()
}

#[allow(clippy::too_many_arguments)]
fn desc(id: u32, name: &str, kind: Kind, len: usize, min: f64, max: f64, def: &[f64], enums: Vec<(i32, String)>) -> ControlDesc {
    ControlDesc { id, name: name.into(), kind, len, min, max, def: def.to_vec(), enums }
}

/// The Pixel 2 XL's libcamera controls.
fn taimen_controls() -> Vec<ControlDesc> {
    use Kind::*;
    vec![
        desc(1, "AeConstraintMode", Int, 1, 0.0, 3.0, &[0.0], enums(&["ConstraintNormal", "ConstraintHighlight", "ConstraintShadows", "ConstraintCustom"])),
        desc(2, "AeEnable", Bool, 1, 0.0, 1.0, &[1.0], vec![]),
        desc(3, "AeExposureMode", Int, 1, 0.0, 3.0, &[0.0], enums(&["ExposureNormal", "ExposureShort", "ExposureLong", "ExposureCustom"])),
        desc(4, "AfMode", Int, 1, 0.0, 2.0, &[2.0], enums(&["AfModeManual", "AfModeAuto", "AfModeContinuous"])),
        desc(5, "AfTrigger", Int, 1, 0.0, 1.0, &[0.0], enums(&["AfTriggerStart", "AfTriggerCancel"])),
        desc(6, "AnalogueGain", Float, 1, 1.0, 16.0, &[1.0], vec![]),
        desc(7, "AnalogueGainMode", Int, 1, 0.0, 1.0, &[0.0], enums(&["AnalogueGainModeAuto", "AnalogueGainModeManual"])),
        desc(8, "AwbEnable", Bool, 1, 0.0, 1.0, &[1.0], vec![]),
        desc(
            9,
            "AwbMode",
            Int,
            1,
            0.0,
            6.0,
            &[0.0],
            enums(&["AwbAuto", "AwbIncandescent", "AwbTungsten", "AwbFluorescent", "AwbIndoor", "AwbDaylight", "AwbCloudy"]),
        ),
        desc(10, "Brightness", Float, 1, -1.0, 1.0, &[0.0], vec![]),
        desc(11, "ColourGains", Float, 2, 0.0, 8.0, &[1.0, 1.0], vec![]),
        desc(12, "Contrast", Float, 1, 0.0, 2.0, &[1.0], vec![]),
        desc(13, "ExposureTime", Int, 1, 100.0, 200000.0, &[16666.0], vec![]),
        desc(14, "ExposureTimeMode", Int, 1, 0.0, 1.0, &[0.0], enums(&["ExposureTimeModeAuto", "ExposureTimeModeManual"])),
        desc(15, "ExposureValue", Float, 1, -2.0, 2.0, &[0.0], vec![]),
        desc(16, "FrameDurationLimits", Int, 2, 8333.0, 200000.0, &[33333.0, 33333.0], vec![]),
        desc(17, "Gamma", Float, 1, 1.0, 4.0, &[2.2], vec![]),
        desc(18, "LensPosition", Float, 1, 0.0, 7.0, &[1.0], vec![]),
        desc(19, "Saturation", Float, 1, 0.0, 2.0, &[1.0], vec![]),
    ]
}

/// A sky, a sun and a lawn, drawn so a 270 degree turn stands them upright.
fn scene(w: u32, h: u32, front: bool) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (y as f32 / h as f32, 1.0 - x as f32 / w as f32);
            let (sx, sy) = (u - 0.7, v - 0.25);
            let px: [u8; 3] = if sx * sx + sy * sy * 0.56 < 0.008 {
                [250, 210, 90]
            } else if v < 0.6 {
                let t = v / 0.6;
                if front { [(120.0 + 60.0 * t) as u8, 90, 140] } else { [(60.0 + 100.0 * t) as u8, (110.0 + 80.0 * t) as u8, (190.0 + 40.0 * t) as u8] }
            } else if (u * 10.0) as i32 % 2 == 0 {
                [70, 110, 60]
            } else {
                [60, 95, 50]
            };
            let i = ((y * w + x) * 4) as usize;
            out[i..i + 4].copy_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    if front {
        // Front buffers want a quarter turn the other way: turn it over.
        let px: Vec<[u8; 4]> = out.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).rev().collect();
        out = px.concat();
    }
    out
}

/// The fake camera, when OBSCURA_FAKE asks for one. It speaks the backend's
/// protocol, including a full-resolution capture's round trip, and logs
/// every control it is sent as `fake-control`.
pub fn backend(emit: impl Fn(Event) + Send + 'static) -> Option<LibcameraBackend> {
    let fake = fake()?;
    let (tx, rx) = std::sync::mpsc::channel::<Internal>();
    let slot = Arc::<FrameSlot>::default();
    let worker_slot = slot.clone();
    std::thread::spawn(move || {
        let infos = vec![
            CameraInfo { id: "back".into(), model: "imx362".into(), facing: Facing::Back, rotation: 0 },
            CameraInfo { id: "front".into(), model: "imx179".into(), facing: Facing::Front, rotation: 0 },
        ];
        if fake == "nocamera" {
            return emit(Event::Cameras(vec![]));
        }
        emit(Event::Cameras(infos.clone()));
        let frame_ms = std::env::var("OBSCURA_FAKE_FRAME_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
        let controls = taimen_controls();
        let (mut open, mut full_resolution): (Option<(usize, Mode, Mode, CameraInfo)>, bool) = (None, true);
        loop {
            match rx.recv_timeout(Duration::from_millis(frame_ms)) {
                Ok(Internal::Cmd(Cmd::Open { .. })) if fake == "busy" => emit(Event::Error("cannot acquire camera: Device or resource busy".into())),
                Ok(Internal::Cmd(Cmd::Open { camera, mode, video })) => {
                    let m = |width, height| Mode { width, height };
                    let modes = if camera == 0 { vec![m(4032, 3032), m(4032, 2272), m(2016, 1512), m(2016, 1136)] } else { vec![m(3280, 2464), m(3280, 1846)] };
                    let mode = mode.filter(|x| modes.contains(x)).unwrap_or(modes[0]);
                    let view = if video { mode } else { crate::camera::viewfinder_mode(&modes, mode) };
                    let mut info = infos[camera].clone();
                    info.rotation = if camera == 0 { 270 } else { 90 };
                    perf!("camera-started", "fake purpose={} view={}x{}", if video { "Video" } else { "Preview" }, view.width, view.height);
                    emit(Event::Opened(Session {
                        camera,
                        info: info.clone(),
                        modes,
                        mode,
                        view,
                        fourcc: u32::from_le_bytes(*b"XB24"),
                        controls: controls.clone(),
                        raw: true,
                        fps: Some((5.0, 120.0)),
                        af_windows: false,
                    }));
                    open = Some((camera, mode, view, info));
                }
                Ok(Internal::Cmd(Cmd::FullResolution(on))) => full_resolution = on,
                Ok(Internal::Cmd(Cmd::SetControl { id, value })) => {
                    let name = controls.iter().find(|c| c.id == id).map_or("?", |c| c.name.as_str());
                    perf!("control", "{name}={value:?}");
                }
                Ok(Internal::Cmd(Cmd::Capture)) => {
                    if let Some((_, mode, view, info)) = &open {
                        if full_resolution && view != mode {
                            perf!("still-reconfigure", "{}x{}", mode.width, mode.height);
                            std::thread::sleep(Duration::from_millis(300));
                            perf!("still-settled", "frames=1");
                        }
                        let (w, h) = (mode.width / 8, mode.height / 8);
                        let front = info.facing == Facing::Front;
                        emit(Event::Still(Box::new(Still {
                            width: w,
                            height: h,
                            stride: w * 4,
                            fourcc: u32::from_le_bytes(*b"XB24"),
                            rgba: scene(w, h, front),
                            raw: None,
                            metadata: Metadata::default(),
                            info: info.clone(),
                            zoom: 1.0,
                        })));
                        if full_resolution && view != mode {
                            perf!("viewfinder-restored");
                        }
                    }
                }
                Ok(Internal::Cmd(Cmd::Close)) => open = None,
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Some((_, _, view, info)) = &open {
                let (w, h) = (view.width / 8, view.height / 8);
                let bytes = scene(w, h, info.facing == Facing::Front);
                worker_slot.deliver(
                    Frame { width: w, height: h, stride: w * 4, fourcc: u32::from_le_bytes(*b"XB24"), fd: -1, offset: 0, bytes: Some(bytes), ret: None },
                    &emit,
                );
                let mut meta = Metadata::default();
                for (k, v) in [
                    ("AnalogueGain", 4.0),
                    ("ExposureTime", 16666.0),
                    ("ColourTemperature", 4600.0),
                    ("LensPosition", 1.2),
                    ("AfState", 2.0),
                    ("FrameDuration", 33333.0),
                ] {
                    meta.values.insert(k.into(), vec![v]);
                }
                meta.values.insert("ColourGains".into(), vec![1.8, 1.5]);
                emit(Event::Metadata(meta));
            }
        }
    });
    Some(LibcameraBackend { tx, slot })
}
