// SPDX-License-Identifier: GPL-3.0-or-later
//! Cameras through PipeWire, the way a sandboxed app reaches them: the Camera
//! portal hands out a PipeWire remote, and GStreamer's PipeWire elements read
//! the camera nodes on it. It speaks the same Cmd/Event protocol as the
//! libcamera backend, with what PipeWire offers: one stream, no per-frame
//! metadata, no raw images, and (for now) no controls. See docs/flatpak.md.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gst::prelude::*;

use crate::camera::{CameraInfo, Cmd, Event, Facing, Frame, FrameSlot, Internal, LibcameraBackend, Metadata, Mode, Session, Still};

/// Whether cameras should come through PipeWire: inside a sandbox, or when
/// OBSCURA_BACKEND=pipewire asks for it.
pub fn wanted() -> bool {
    match std::env::var("OBSCURA_BACKEND").as_deref() {
        Ok("pipewire") => true,
        Ok(_) => false,
        Err(_) => ashpd::is_sandboxed(),
    }
}

pub fn spawn(remote: Option<OwnedFd>, emit: impl Fn(Event) + Send + Sync + 'static) -> LibcameraBackend {
    let (tx, rx) = channel();
    let slot = Arc::<FrameSlot>::default();
    let worker_slot = slot.clone();
    std::thread::Builder::new().name("pipewire".into()).spawn(move || run(remote, rx, worker_slot, Arc::new(emit))).expect("spawn pipewire thread");
    LibcameraBackend { tx, slot }
}

/// DRM fourccs for the raw formats the viewfinder, JPEG and recorder take.
fn fourcc(format: gst_video::VideoFormat) -> Option<u32> {
    use gst_video::VideoFormat::*;
    Some(u32::from_le_bytes(*match format {
        Nv12 => b"NV12",
        Rgbx => b"XB24",
        Rgba => b"AB24",
        Bgrx => b"XR24",
        Bgra => b"AR24",
        _ => return None,
    }))
}

/// GStreamer's image-orientation tag: degrees to turn the picture clockwise.
fn orientation_degrees(tag: &str) -> Option<i32> {
    Some(match tag {
        "rotate-0" => 0,
        "rotate-90" => 90,
        "rotate-180" => 180,
        "rotate-270" => 270,
        _ => return None,
    })
}

/// Distinct raw frame sizes in a device's caps, largest first.
fn modes(caps: &gst::CapsRef) -> Vec<Mode> {
    let mut out: Vec<Mode> = caps
        .iter()
        .filter(|s| s.name() == "video/x-raw")
        .filter_map(|s| Some(Mode { width: s.get::<i32>("width").ok()? as u32, height: s.get::<i32>("height").ok()? as u32 }))
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.width as u64 * m.height as u64));
    out.dedup();
    out
}

fn default_mode(modes: &[Mode]) -> Option<Mode> {
    modes.iter().copied().filter(|m| m.width <= 1920).max_by_key(|m| m.width as u64 * m.height as u64).or_else(|| modes.last().copied())
}

struct Shared {
    emit: Arc<dyn Fn(Event) + Send + Sync>,
    slot: Arc<FrameSlot>,
    info: Mutex<Option<CameraInfo>>,
    capture: AtomicBool,
    recorder: Mutex<Option<Arc<crate::video::Recorder>>>,
    rotation: AtomicI32,
    first: AtomicBool,
}

fn run(remote: Option<OwnedFd>, rx: Receiver<Internal>, slot: Arc<FrameSlot>, emit: Arc<dyn Fn(Event) + Send + Sync>) {
    crate::video::gst();
    let Some(provider) = gst::DeviceProviderFactory::by_name("pipewiredeviceprovider") else {
        return emit(Event::Error("GStreamer's PipeWire plugin is missing".into()));
    };
    if let Some(fd) = &remote {
        provider.set_property("fd", fd.as_raw_fd());
    }
    if provider.start().is_err() {
        return emit(Event::Error("cannot connect to PipeWire".into()));
    }
    let devices: Vec<gst::Device> = provider.devices().into_iter().filter(|d| d.device_class() == "Video/Source").collect();
    perf!("camera-manager", "pipewire cameras={}", devices.len());
    let infos: Vec<CameraInfo> = devices
        .iter()
        .map(|d| {
            let props = d.properties();
            let prop = |k: &str| props.as_ref().and_then(|p| p.get::<String>(k).ok());
            CameraInfo {
                id: prop("object.serial").or_else(|| prop("node.name")).unwrap_or_else(|| d.display_name().to_string()),
                model: d.display_name().to_string(),
                facing: match prop("api.libcamera.location").as_deref() {
                    Some("front") => Facing::Front,
                    Some("back") => Facing::Back,
                    _ => Facing::External,
                },
                rotation: 0,
            }
        })
        .collect();
    emit(Event::Cameras(infos.clone()));

    let shared = Arc::new(Shared {
        emit: emit.clone(),
        slot,
        info: Mutex::new(None),
        capture: AtomicBool::new(false),
        recorder: Mutex::new(None),
        rotation: AtomicI32::new(0),
        first: AtomicBool::new(true),
    });
    #[cfg(feature = "pipewire-controls")]
    let props = props::spawn(remote.as_ref().and_then(|fd| fd.try_clone().ok()), emit.clone());
    let mut pipeline: Option<gst::Pipeline> = None;
    while let Ok(msg) = rx.recv() {
        match msg {
            Internal::Cmd(Cmd::Open { camera, mode, .. }) => {
                if let Some(p) = pipeline.take() {
                    let _ = p.set_state(gst::State::Null);
                }
                let Some(device) = devices.get(camera) else {
                    emit(Event::Error(format!("no camera {camera}")));
                    continue;
                };
                match open(device, camera, infos[camera].clone(), mode, &shared) {
                    Ok((p, session)) => {
                        emit(Event::Opened(session));
                        pipeline = Some(p);
                        #[cfg(feature = "pipewire-controls")]
                        if let Some(props) = &props {
                            let _ = props.send(props::Request::Watch(infos[camera].id.clone()));
                        }
                    }
                    Err(e) => emit(Event::Error(e)),
                }
            }
            Internal::Cmd(Cmd::Capture) => shared.capture.store(true, Ordering::Relaxed),
            Internal::Cmd(Cmd::Record(r)) => *shared.recorder.lock().unwrap() = r,
            Internal::Cmd(Cmd::Close) => {
                if let Some(p) = pipeline.take() {
                    let _ = p.set_state(gst::State::Null);
                }
            }
            #[cfg(feature = "pipewire-controls")]
            Internal::Cmd(Cmd::SetControl { id, value }) => {
                if let Some(props) = &props {
                    let _ = props.send(props::Request::Set(id, value));
                }
            }
            // Without the pipewire-controls feature the node's controls
            // stay untouched: GStreamer's source does not expose them.
            Internal::Cmd(_) | Internal::Done(_) | Internal::Returned(..) => {}
        }
    }
    if let Some(p) = pipeline {
        let _ = p.set_state(gst::State::Null);
    }
    drop(remote);
}

fn open(device: &gst::Device, index: usize, info: CameraInfo, mode: Option<Mode>, shared: &Arc<Shared>) -> Result<(gst::Pipeline, Session), String> {
    let caps = device.caps().ok_or("the camera reports no formats")?;
    let modes = modes(&caps);
    // One stream carries viewfinder, photos and video, and every frame is
    // copied: start at the largest size up to 1080p wide, and let the
    // resolution menu offer the rest.
    let mode = mode.filter(|m| modes.contains(m)).or_else(|| default_mode(&modes)).ok_or("the camera reports no frame sizes")?;
    let src = device.create_element(None).map_err(|e| format!("PipeWire source: {e}"))?;
    let size = gst::Caps::builder("video/x-raw").field("width", mode.width as i32).field("height", mode.height as i32).build();
    let filter = gst::ElementFactory::make("capsfilter").property("caps", &size).build().map_err(|e| e.to_string())?;
    let convert = gst::ElementFactory::make("videoconvert").build().map_err(|e| e.to_string())?;
    let formats = gst::Caps::builder("video/x-raw").field("format", gst::List::new(["NV12", "RGBx", "BGRx", "RGBA", "BGRA"])).build();
    let sink = gst_app::AppSink::builder().caps(&formats).max_buffers(1).drop(true).sync(false).build();
    let pipeline = gst::Pipeline::new();
    pipeline.add_many([&src, &filter, &convert, sink.upcast_ref()]).map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &filter, &convert, sink.upcast_ref()]).map_err(|e| e.to_string())?;

    // PipeWire's transform meta arrives as an image-orientation tag.
    shared.rotation.store(0, Ordering::Relaxed);
    if let Some(pad) = sink.static_pad("sink") {
        let s = shared.clone();
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, probe| {
            if let Some(gst::PadProbeData::Event(event)) = &probe.data
                && let gst::EventView::Tag(tag) = event.view()
                && let Some(value) = tag.tag().get::<gst::tags::ImageOrientation>()
                && let Some(degrees) = orientation_degrees(value.get())
            {
                s.rotation.store(degrees, Ordering::Relaxed);
            }
            gst::PadProbeReturn::Ok
        });
    }

    let s = shared.clone();
    sink.set_callbacks(gst_app::AppSinkCallbacks::builder().new_sample(move |sink| sample(sink, &s)).build());
    shared.first.store(true, Ordering::Relaxed);
    *shared.info.lock().unwrap() = Some(info.clone());
    pipeline.set_state(gst::State::Playing).map_err(|_| "cannot start the PipeWire stream".to_string())?;
    // The first caps settle the format, and the tag the rotation, before the
    // interface hears of the session.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let negotiated = loop {
        if let Some(caps) = sink.static_pad("sink").and_then(|p| p.current_caps()) {
            break caps;
        }
        if std::time::Instant::now() > deadline {
            let _ = pipeline.set_state(gst::State::Null);
            return Err("the PipeWire stream did not start".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let video = gst_video::VideoInfo::from_caps(&negotiated).map_err(|e| e.to_string())?;
    let mut info = info;
    info.rotation = shared.rotation.load(Ordering::Relaxed);
    *shared.info.lock().unwrap() = Some(info.clone());
    let fps = negotiated.structure(0).and_then(|s| s.get::<gst::Fraction>("framerate").ok()).map(|f| f.numer() as f64 / f.denom().max(1) as f64);
    perf!("camera-started", "pipewire view={}x{} format={:?}", video.width(), video.height(), video.format());
    let session = Session {
        camera: index,
        info,
        modes,
        mode,
        view: Mode { width: video.width(), height: video.height() },
        fourcc: fourcc(video.format()).unwrap_or(0),
        controls: Vec::new(),
        raw: false,
        fps: fps.filter(|f| *f > 0.0).map(|f| (f, f)),
        af_windows: false,
        metadata: false,
    };
    Ok((pipeline, session))
}

fn sample(sink: &gst_app::AppSink, s: &Shared) -> Result<gst::FlowSuccess, gst::FlowError> {
    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else { return Ok(gst::FlowSuccess::Ok) };
    let Ok(video) = gst_video::VideoInfo::from_caps(caps) else { return Ok(gst::FlowSuccess::Ok) };
    let Some(code) = fourcc(video.format()) else { return Ok(gst::FlowSuccess::Ok) };
    let stride = buffer.meta::<gst_video::VideoMeta>().map_or(video.stride()[0], |m| m.stride()[0]) as u32;
    let Ok(map) = buffer.map_readable() else { return Ok(gst::FlowSuccess::Ok) };
    let (width, height) = (video.width(), video.height());
    if let Some(r) = s.recorder.lock().unwrap().as_ref() {
        r.push(map.as_slice(), stride);
    }
    if s.capture.swap(false, Ordering::Relaxed)
        && let Some(info) = s.info.lock().unwrap().clone()
    {
        let info = CameraInfo { rotation: s.rotation.load(Ordering::Relaxed), ..info };
        (s.emit)(Event::Still(Box::new(Still {
            width,
            height,
            stride,
            fourcc: code,
            rgba: map.to_vec(),
            raw: None,
            metadata: Metadata::default(),
            info,
            zoom: 1.0,
        })));
    }
    if s.first.swap(false, Ordering::Relaxed) {
        perf!("frame-first-queued");
    }
    let frame = Frame { width, height, stride, fourcc: code, fd: -1, offset: 0, bytes: Some(map.to_vec()), ret: None };
    s.slot.deliver(frame, &*s.emit);
    Ok(gst::FlowSuccess::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_orientation_from_gstreamer() {
        crate::video::gst();
        let caps: gst::Caps = "video/x-raw,format=NV12,width=640,height=480; video/x-raw,format=NV12,width=1920,height=1080; video/x-raw,format=YUY2,width=640,height=480; image/jpeg,width=4000,height=3000".parse().unwrap();
        assert_eq!(modes(&caps), [Mode { width: 1920, height: 1080 }, Mode { width: 640, height: 480 }]);
        assert_eq!(default_mode(&modes(&caps)), Some(Mode { width: 1920, height: 1080 }));
        assert_eq!(default_mode(&[Mode { width: 4032, height: 3024 }]), Some(Mode { width: 4032, height: 3024 }));
        assert_eq!(orientation_degrees("rotate-270"), Some(270));
        assert_eq!(orientation_degrees("flip-rotate-90"), None);
        assert_eq!(fourcc(gst_video::VideoFormat::Rgbx), Some(u32::from_le_bytes(*b"XB24")));
    }
}

/// Camera controls on the PipeWire node: PipeWire's libcamera plugin
/// publishes the camera's single-value controls as PropInfo params and takes
/// changes as Props. A second PipeWire connection on its own thread watches
/// the node the stream uses.
#[cfg(feature = "pipewire-controls")]
mod props {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use pipewire as pw;
    use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
    use pw::spa::utils::ChoiceEnum;

    use crate::camera::{ControlDesc, Event, Kind};

    pub enum Request {
        /// Follow the node with this object.serial.
        Watch(String),
        Set(u32, Vec<f64>),
    }

    pub fn spawn(remote: Option<std::os::fd::OwnedFd>, emit: Arc<dyn Fn(Event) + Send + Sync>) -> Option<pw::channel::Sender<Request>> {
        let (tx, rx) = pw::channel::channel::<Request>();
        std::thread::Builder::new()
            .name("pipewire-props".into())
            .spawn(move || {
                if let Err(e) = run(remote, rx, emit) {
                    log::warn!("PipeWire controls: {e}");
                }
            })
            .ok()?;
        Some(tx)
    }

    /// A PropInfo param as a control the panel can show.
    pub fn describe(value: &Value) -> Option<ControlDesc> {
        let Value::Object(object) = value else { return None };
        let find = |key: u32| object.properties.iter().find(|p| p.key == key).map(|p| &p.value);
        let Value::Id(id) = find(pw::spa::sys::SPA_PROP_INFO_id)? else { return None };
        let Value::String(name) = find(pw::spa::sys::SPA_PROP_INFO_description)? else { return None };
        let mut enums = Vec::new();
        if let Some(Value::Struct(labels)) = find(pw::spa::sys::SPA_PROP_INFO_labels) {
            for pair in labels.chunks(2) {
                if let [Value::Int(v), Value::String(label)] = pair {
                    enums.push((*v, label.clone()));
                }
            }
        }
        let (kind, min, max, def) = match find(pw::spa::sys::SPA_PROP_INFO_type)? {
            Value::Choice(ChoiceValue::Float(c)) => match &c.1 {
                ChoiceEnum::Range { default, min, max } => (Kind::Float, *min as f64, *max as f64, *default as f64),
                ChoiceEnum::None(v) => (Kind::Float, *v as f64, *v as f64, *v as f64),
                _ => return None,
            },
            Value::Choice(ChoiceValue::Int(c)) => match &c.1 {
                ChoiceEnum::Range { default, min, max } => (Kind::Int, *min as f64, *max as f64, *default as f64),
                ChoiceEnum::Enum { default, alternatives } => {
                    let lo = alternatives.iter().copied().min().unwrap_or(*default);
                    let hi = alternatives.iter().copied().max().unwrap_or(*default);
                    (Kind::Int, lo as f64, hi as f64, *default as f64)
                }
                ChoiceEnum::None(v) => (Kind::Int, *v as f64, *v as f64, *v as f64),
                _ => return None,
            },
            Value::Choice(ChoiceValue::Bool(c)) => match &c.1 {
                ChoiceEnum::Enum { default, .. } | ChoiceEnum::None(default) => (Kind::Bool, 0.0, 1.0, *default as u8 as f64),
                _ => return None,
            },
            _ => return None,
        };
        Some(ControlDesc { id: id.0, name: name.clone(), kind, len: 1, min, max, def: vec![def], enums })
    }

    /// A Props param setting one control.
    pub fn props(id: u32, kind: Kind, value: &[f64]) -> Option<Vec<u8>> {
        let v = *value.first()?;
        let value = match kind {
            Kind::Bool => Value::Bool(v != 0.0),
            Kind::Int => Value::Int(v.round() as i32),
            Kind::Float => Value::Float(v as f32),
            Kind::Other => return None,
        };
        let object = Object { type_: pw::spa::sys::SPA_TYPE_OBJECT_Props, id: pw::spa::sys::SPA_PARAM_Props, properties: vec![Property::new(id, value)] };
        pw::spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object)).ok().map(|(c, _)| c.into_inner())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use pw::spa::pod::deserialize::PodDeserializer;
        use pw::spa::utils::{Choice, ChoiceFlags, Id};

        fn info(id: u32, name: &str, kind: Value, labels: Option<Vec<Value>>) -> Value {
            let mut properties = vec![
                Property::new(pw::spa::sys::SPA_PROP_INFO_id, Value::Id(Id(id))),
                Property::new(pw::spa::sys::SPA_PROP_INFO_description, Value::String(name.into())),
                Property::new(pw::spa::sys::SPA_PROP_INFO_type, kind),
            ];
            if let Some(l) = labels {
                properties.push(Property::new(pw::spa::sys::SPA_PROP_INFO_labels, Value::Struct(l)));
            }
            Value::Object(Object { type_: pw::spa::sys::SPA_TYPE_OBJECT_PropInfo, id: pw::spa::sys::SPA_PARAM_PropInfo, properties })
        }

        #[test]
        fn prop_infos_become_controls() {
            let exposure = info(
                0x100_0010,
                "ExposureTime",
                Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 20000, min: 100, max: 66666 }))),
                None,
            );
            let d = describe(&exposure).unwrap();
            assert_eq!((d.name.as_str(), d.kind, d.min, d.max, d.def[0]), ("ExposureTime", Kind::Int, 100.0, 66666.0, 20000.0));

            let af = info(
                0x100_0020,
                "AfMode",
                Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Enum { default: 2, alternatives: vec![0, 1, 2] }))),
                Some(vec![Value::Int(0), Value::String("AfModeManual".into()), Value::Int(2), Value::String("AfModeContinuous".into())]),
            );
            let d = describe(&af).unwrap();
            assert_eq!(d.enums, vec![(0, "AfModeManual".to_string()), (2, "AfModeContinuous".to_string())]);
            assert_eq!((d.min, d.max, d.def[0]), (0.0, 2.0, 2.0));

            let ae = info(
                0x100_0030,
                "AeEnable",
                Value::Choice(ChoiceValue::Bool(Choice(ChoiceFlags::empty(), ChoiceEnum::Enum { default: true, alternatives: vec![false, true] }))),
                None,
            );
            assert_eq!(describe(&ae).unwrap().kind, Kind::Bool);
        }

        #[test]
        fn props_round_trip() {
            let bytes = props(0x100_0010, Kind::Float, &[0.5]).unwrap();
            let (_, value) = PodDeserializer::deserialize_any_from(&bytes).unwrap();
            let Value::Object(o) = value else { panic!() };
            assert_eq!((o.type_, o.id), (pw::spa::sys::SPA_TYPE_OBJECT_Props, pw::spa::sys::SPA_PARAM_Props));
            assert_eq!(o.properties[0].key, 0x100_0010);
            assert_eq!(o.properties[0].value, Value::Float(0.5));
        }
    }

    struct Entry {
        node: pw::node::Node,
        _listener: pw::node::NodeListener,
        controls: HashMap<u32, ControlDesc>,
    }

    #[derive(Default)]
    struct State {
        /// The object.serial of the node the stream uses.
        wanted: Option<String>,
        /// Every camera node, bound as it appears, so its controls are known
        /// by the time a stream picks it.
        nodes: HashMap<String, Entry>,
    }

    fn run(remote: Option<std::os::fd::OwnedFd>, rx: pw::channel::Receiver<Request>, emit: Arc<dyn Fn(Event) + Send + Sync>) -> Result<(), pw::Error> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = match remote {
            Some(fd) => context.connect_fd_rc(fd, None)?,
            None => context.connect_rc(None)?,
        };
        let registry = core.get_registry_rc()?;
        let state = Rc::new(RefCell::new(State::default()));

        // Every new control announces the watched node's list; the
        // interface coalesces the burst.
        let announce = {
            let emit = emit.clone();
            Rc::new(move |s: &State| {
                let Some(entry) = s.wanted.as_ref().and_then(|w| s.nodes.get(w)) else { return };
                let mut list: Vec<ControlDesc> = entry.controls.values().cloned().collect();
                list.sort_by(|a, b| a.name.cmp(&b.name));
                emit(Event::Controls(list));
            })
        };

        let _registry_listener = {
            let (state, registry2, announce) = (state.clone(), registry.clone(), announce.clone());
            registry
                .add_listener_local()
                .global(move |global| {
                    let props = global.props.as_ref();
                    if global.type_ != pw::types::ObjectType::Node || props.and_then(|p| p.get("media.class")) != Some("Video/Source") {
                        return;
                    }
                    let Some(serial) = props.and_then(|p| p.get("object.serial")).map(str::to_string) else { return };
                    let Ok(node) = registry2.bind::<pw::node::Node, _>(global) else { return };
                    let (state3, announce, key) = (state.clone(), announce.clone(), serial.clone());
                    let listener = node
                        .add_listener_local()
                        .param(move |_, kind, _, _, pod| {
                            if kind != pw::spa::param::ParamType::PropInfo {
                                return;
                            }
                            let Some(pod) = pod else { return };
                            let Ok((_, value)) = pw::spa::pod::deserialize::PodDeserializer::deserialize_any_from(pod.as_bytes()) else { return };
                            let Some(desc) = describe(&value) else { return };
                            let mut s = state3.borrow_mut();
                            if let Some(entry) = s.nodes.get_mut(&key) {
                                entry.controls.insert(desc.id, desc);
                            }
                            if s.wanted.as_deref() == Some(key.as_str()) {
                                announce(&s);
                            }
                        })
                        .register();
                    node.subscribe_params(&[pw::spa::param::ParamType::PropInfo]);
                    state.borrow_mut().nodes.insert(serial, Entry { node, _listener: listener, controls: HashMap::new() });
                })
                .register()
        };

        let _receiver = {
            let (state, announce) = (state.clone(), announce.clone());
            rx.attach(mainloop.loop_(), move |request| match request {
                Request::Watch(serial) => {
                    state.borrow_mut().wanted = Some(serial);
                    announce(&state.borrow());
                }
                Request::Set(id, value) => {
                    let s = state.borrow();
                    if let Some(entry) = s.wanted.as_ref().and_then(|w| s.nodes.get(w))
                        && let Some(desc) = entry.controls.get(&id)
                        && let Some(bytes) = props(id, desc.kind, &value)
                        && let Some(pod) = Pod::from_bytes(&bytes)
                    {
                        entry.node.set_param(pw::spa::param::ParamType::Props, 0, pod);
                        perf!("control", "{}={value:?}", desc.name);
                    }
                }
            })
        };
        mainloop.run();
        Ok(())
    }
}
