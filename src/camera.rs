// SPDX-License-Identifier: GPL-3.0-or-later
//! Capture backends. The UI talks to a backend only through [`Cmd`] and
//! [`Event`], so a PipeWire backend (for a sandboxed build, which gets a
//! PipeWire remote from the camera portal instead of device access) can sit
//! next to the libcamera one without the UI knowing which it has.

use std::collections::HashMap;

use gettextrs::gettext;
use std::ffi::CStr;
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use libcamera::camera::{ActiveCamera, CameraConfiguration, CameraConfigurationStatus, Orientation};
use libcamera::camera_manager::CameraManager;
use libcamera::control::ControlList;
use libcamera::control_value::ControlValue;
use libcamera::framebuffer::AsFrameBuffer;
use libcamera::framebuffer_allocator::{FrameBuffer, FrameBufferAllocator};
use libcamera::framebuffer_map::MemoryMappedFrameBuffer;
use libcamera::geometry::Size;
use libcamera::pixel_format::PixelFormat;
use libcamera::properties;
use libcamera::request::{Request, ReuseFlag};
use libcamera::stream::{Stream, StreamRole};
use libcamera::utils::UniquePtr;
use smallvec::SmallVec;

pub type Buffer = MemoryMappedFrameBuffer<FrameBuffer>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Front,
    Back,
    External,
}

#[derive(Debug, Clone)]
pub struct CameraInfo {
    pub id: String,
    pub model: String,
    pub facing: Facing,
    /// Degrees the buffers must be turned clockwise to be upright. Known once
    /// the camera is configured; 0 before.
    pub rotation: i32,
}

impl CameraInfo {
    /// What to call the camera in the interface.
    pub fn name(&self) -> String {
        match self.facing {
            Facing::Back => gettext("Back Camera"),
            Facing::Front => gettext("Front Camera"),
            Facing::External => self.model.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    /// Anything a generated widget cannot edit (rectangles, sizes, strings).
    Other,
}

/// One entry of the camera's ControlInfoMap, flattened for the UI.
#[derive(Debug, Clone)]
pub struct ControlDesc {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    /// Elements per value: 1 for a scalar, N for a fixed array, 0 if dynamic.
    pub len: usize,
    pub min: f64,
    pub max: f64,
    pub def: Vec<f64>,
    /// Named values, for enum-like integer controls.
    pub enums: Vec<(i32, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub camera: usize,
    pub info: CameraInfo,
    pub modes: Vec<Mode>,
    pub mode: Mode,
    pub view: Mode,
    /// DRM fourcc of the viewfinder stream.
    pub fourcc: u32,
    pub controls: Vec<ControlDesc>,
    pub raw: bool,
    /// Frame rate range the configured mode allows, if the camera reports it.
    pub fps: Option<(f64, f64)>,
    /// Autofocus can be pointed at a spot (AfWindows).
    pub af_windows: bool,
}

/// A viewfinder frame. The dmabuf stays valid, and the request stays out of
/// the camera's queue, until this is dropped.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub fourcc: u32,
    pub fd: RawFd,
    pub offset: u32,
    /// CPU copy, only when the backend was told the dmabuf path failed.
    pub bytes: Option<Vec<u8>>,
    pub(crate) ret: Option<(Request, Sender<Internal>, u64)>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{} {:08x})", self.width, self.height, self.fourcc)
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let Some((req, tx, generation)) = self.ret.take() {
            let _ = tx.send(Internal::Returned(req, generation));
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// libcamera format name, e.g. "SRGGB10_CSI2P".
    pub format: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub values: HashMap<String, Vec<f64>>,
}

impl Metadata {
    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).and_then(|v| v.first().copied())
    }
}

#[derive(Debug, Clone)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub fourcc: u32,
    pub rgba: Vec<u8>,
    pub raw: Option<RawImage>,
    pub metadata: Metadata,
    pub info: CameraInfo,
    /// Digital zoom to crop to when saving.
    pub zoom: f64,
}

#[derive(Debug)]
pub enum Cmd {
    /// `mode` is the photo (or video) size; for photos the viewfinder may
    /// run a smaller, faster mode of the same shape.
    Open { camera: usize, mode: Option<Mode>, video: bool },
    /// Photos from the full sensor mode (a quick reconfiguration per shot)
    /// rather than from the viewfinder.
    FullResolution(bool),
    SetControl { id: u32, value: Vec<f64> },
    /// Meter autofocus around a point, in 0..1 sensor coordinates.
    FocusAt(f64, f64),
    Capture,
    Record(Option<std::sync::Arc<crate::video::Recorder>>),
    CopyFrames(bool),
    Close,
}

#[derive(Debug)]
pub enum Event {
    Cameras(Vec<CameraInfo>),
    Opened(Session),
    /// A new viewfinder frame is waiting in the backend's slot.
    FrameReady,
    Metadata(Metadata),
    Still(Box<Still>),
    Error(String),
}

pub trait Backend {
    fn send(&self, cmd: Cmd);
}

pub enum Internal {
    Cmd(Cmd),
    Done(Request),
    /// A request whose frame the UI let go of, tagged with the session it
    /// came from: one that outlives its session must not be requeued into
    /// the next one.
    Returned(Request, u64),
}

pub struct LibcameraBackend {
    pub(crate) tx: Sender<Internal>,
    pub(crate) slot: Arc<FrameSlot>,
}

/// The newest viewfinder frame, and nothing older: a frame the interface has
/// not taken by the time the next one completes goes straight back to the
/// camera, so a busy main thread costs a skipped frame, never latency or
/// buffers stuck in a queue.
#[derive(Default)]
pub struct FrameSlot {
    frame: std::sync::Mutex<Option<Frame>>,
    /// Frames replaced before the interface took them.
    replaced: std::sync::atomic::AtomicU32,
}

impl FrameSlot {
    pub fn deliver(&self, frame: Frame, emit: &impl Fn(Event)) {
        let stale = self.frame.lock().unwrap().replace(frame);
        match stale {
            None => emit(Event::FrameReady),
            Some(_) => {
                self.replaced.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

impl Backend for LibcameraBackend {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(Internal::Cmd(cmd));
    }
}

impl LibcameraBackend {
    pub fn spawn(emit: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let worker_tx = tx.clone();
        let slot = Arc::<FrameSlot>::default();
        let worker_slot = slot.clone();
        std::thread::Builder::new()
            .name("libcamera".into())
            .spawn(move || run(rx, worker_tx, worker_slot, emit))
            .expect("spawn libcamera thread");
        Self { tx, slot }
    }

    pub fn take_frame(&self) -> Option<Frame> {
        self.slot.frame.lock().unwrap().take()
    }

    /// Frames skipped since the last call.
    pub fn take_replaced(&self) -> u32 {
        self.slot.replaced.swap(0, std::sync::atomic::Ordering::Relaxed)
    }
}

/// Viewfinder formats in order of preference. libcamera pixel formats are DRM
/// fourccs, which is what GdkDmabufTextureBuilder wants too.
const VIEW_FORMATS: &[&[u8; 4]] = &[b"XB24", b"AB24", b"XR24", b"AR24", b"NV12", b"YUYV"];

fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

fn run(rx: Receiver<Internal>, tx: Sender<Internal>, slot: Arc<FrameSlot>, emit: impl Fn(Event)) {
    let mgr = match CameraManager::new() {
        Ok(mgr) => mgr,
        Err(e) => return emit(Event::Error(format!("libcamera: {e}"))),
    };
    let cameras = mgr.cameras();
    perf!("camera-manager", "cameras={}", cameras.len());
    let infos: Vec<CameraInfo> = (0..cameras.len())
        .filter_map(|i| cameras.get(i))
        .map(|cam| {
            let props = cam.properties();
            CameraInfo {
                id: cam.id().to_string(),
                model: props
                    .get::<properties::Model>()
                    .map(|m| m.0.chars().filter(|c| !c.is_control()).collect::<String>())
                    .ok()
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| cam.id().to_string()),
                facing: match props.get::<properties::Location>() {
                    Ok(properties::Location::CameraFront) => Facing::Front,
                    Ok(properties::Location::CameraBack) => Facing::Back,
                    _ => Facing::External,
                },
                rotation: 0,
            }
        })
        .collect();
    emit(Event::Cameras(infos.clone()));

    let mut session: Option<Live> = None;
    let mut copy_frames = false;
    let mut full_resolution = true;
    // Commands that arrived while a session was closing.
    let mut backlog = std::collections::VecDeque::new();

    while let Some(msg) = backlog.pop_front().or_else(|| rx.recv().ok()) {
        match msg {
            Internal::Cmd(Cmd::Open { camera, mode, video }) => {
                // The same camera stays acquired: a new mode is a reconfigure.
                let kept = session.take().and_then(|live| {
                    let index = live.info.camera;
                    let (cam, deferred) = live.close(&rx, &slot);
                    backlog.extend(deferred);
                    (index == camera).then_some(cam)
                });
                let cam = match kept.map(Ok).unwrap_or_else(|| acquire(&cameras, camera)) {
                    Ok(cam) => cam,
                    Err(e) => {
                        emit(Event::Error(e));
                        continue;
                    }
                };
                let purpose = if video { Purpose::Video } else { Purpose::Preview };
                match Live::open(cam, camera, infos[camera].clone(), mode, purpose, &[], &tx) {
                    Ok((live, info)) => {
                        emit(Event::Opened(info));
                        session = Some(live);
                    }
                    Err(e) => emit(Event::Error(e)),
                }
            }
            Internal::Cmd(Cmd::FullResolution(on)) => full_resolution = on,
            Internal::Cmd(Cmd::SetControl { id, value }) => {
                if let Some(live) = session.as_mut() {
                    live.set_control(id, &value);
                }
            }
            Internal::Cmd(Cmd::FocusAt(x, y)) => {
                if let Some(live) = session.as_mut() {
                    live.focus_at(x, y);
                }
            }
            Internal::Cmd(Cmd::Capture) => match session.take() {
                // The viewfinder runs a smaller mode: switch to the photo mode
                // with the viewfinder's exposure, white balance and focus held
                // manually, take the first frame that has them, switch back.
                Some(live) if full_resolution && live.purpose == Purpose::Preview && live.view_mode != live.info.mode => {
                    perf!("still-reconfigure", "{}x{}", live.info.mode.width, live.info.mode.height);
                    let (index, info, mode, user) = (live.info.camera, live.info.info.clone(), live.info.mode, live.user.clone());
                    let held = seed(&live.info.controls, &live.meta);
                    let expect = (live.meta.get("ExposureTime"), live.meta.get("AnalogueGain"));
                    let (cam, deferred) = live.close(&rx, &slot);
                    backlog.extend(deferred);
                    let initial: Vec<_> = user.iter().cloned().chain(held).collect();
                    match Live::open(cam, index, info.clone(), Some(mode), Purpose::Still, &initial, &tx) {
                        Ok((mut still, _)) => {
                            still.capture_next = true;
                            still.expect = expect;
                            still.user = user;
                            session = Some(still);
                        }
                        Err(e) => {
                            emit(Event::Error(e));
                            session = acquire(&cameras, index).and_then(|cam| Live::open(cam, index, info, Some(mode), Purpose::Preview, &user, &tx)).ok().map(|(l, _)| l);
                        }
                    }
                }
                Some(mut live) => {
                    live.capture_next = true;
                    session = Some(live);
                }
                None => {}
            },
            Internal::Cmd(Cmd::Record(recorder)) => {
                if let Some(live) = session.as_mut() {
                    live.recorder = recorder;
                }
            }
            Internal::Cmd(Cmd::CopyFrames(on)) => copy_frames = on,
            Internal::Cmd(Cmd::Close) => {
                if let Some(live) = session.take() {
                    backlog.extend(live.close(&rx, &slot).1);
                }
            }
            Internal::Done(req) => match session.as_mut() {
                Some(live) => {
                    live.completed(req, copy_frames, &tx, &slot, &emit);
                    // The photo is taken: back to the fast viewfinder, with
                    // the user's own settings.
                    if live.purpose == Purpose::Still && !live.capture_next {
                        let live = session.take().unwrap();
                        let (index, info, mode, user) = (live.info.camera, live.info.info.clone(), live.info.mode, live.user.clone());
                        let (cam, deferred) = live.close(&rx, &slot);
                        backlog.extend(deferred);
                        match Live::open(cam, index, info, Some(mode), Purpose::Preview, &user, &tx) {
                            Ok((live, _)) => {
                                perf!("viewfinder-restored");
                                session = Some(live);
                            }
                            Err(e) => emit(Event::Error(e)),
                        }
                    }
                }
                None => drop(req),
            },
            Internal::Returned(req, generation) => match session.as_mut() {
                Some(live) if live.generation == generation => live.requeue(req),
                _ => {
                    perf!("late-return", "generation={generation}");
                    drop(req)
                }
            },
        }
    }
}

struct Live {
    cam: ActiveCamera<'static>,
    // Dropped after the camera stops and every request is back.
    _alloc: FrameBufferAllocator,
    view: Stream,
    raw: Option<Stream>,
    info: Session,
    pending: UniquePtr<ControlList>,
    capture_next: bool,
    recorder: Option<std::sync::Arc<crate::video::Recorder>>,
    generation: u64,
    /// Nothing delivered yet in this session (for timing).
    first_frame: bool,
    last_meta: Instant,
    outstanding: usize,
    /// The frame AfWindows are expressed in.
    crop_max: Option<libcamera::geometry::Rectangle>,
    purpose: Purpose,
    /// What the viewfinder stream runs at; `info.mode` is the photo size.
    view_mode: Mode,
    /// Every control the user set, in order, to carry across reconfigurations.
    user: Vec<(u32, Vec<f64>)>,
    /// The newest metadata sent to the interface.
    meta: Metadata,
    /// A still session waits for these exposure and gain before shooting.
    expect: (Option<f64>, Option<f64>),
    frames_seen: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// A viewfinder for photos: the fastest mode of the photo's shape.
    Preview,
    /// One photo at the full photo mode, then back to Preview.
    Still,
    /// Viewfinder and recording at the chosen mode.
    Video,
}

/// The smallest mode of `photo`'s shape that is still sharp on a phone or
/// laptop screen; binned modes also run faster.
pub(crate) fn viewfinder_mode(modes: &[Mode], photo: Mode) -> Mode {
    let ratio = |m: &Mode| m.width as f64 / m.height.max(1) as f64;
    modes
        .iter()
        .copied()
        .filter(|m| (ratio(m) - ratio(&photo)).abs() < 0.03 && m.width <= photo.width && m.width >= photo.width.min(1920))
        .min_by_key(|m| m.width)
        .unwrap_or(photo)
}

/// Manual controls that hold what the camera chose automatically in `meta`,
/// for the controls this camera has.
fn seed(controls: &[ControlDesc], meta: &Metadata) -> Vec<(u32, Vec<f64>)> {
    let find = |name: &str| controls.iter().find(|c| c.name == name);
    let manual = |name: &str| find(name).and_then(|c| c.enums.iter().find(|(_, n)| n.ends_with("Manual")).map(|(v, _)| (c.id, vec![*v as f64])));
    let mut out = Vec::new();
    if let (Some(e), Some(g)) = (meta.get("ExposureTime"), meta.get("AnalogueGain")) {
        out.extend(find("AeEnable").map(|c| (c.id, vec![0.0])));
        out.extend(manual("ExposureTimeMode"));
        out.extend(manual("AnalogueGainMode"));
        out.extend(find("ExposureTime").map(|c| (c.id, vec![e])));
        out.extend(find("AnalogueGain").map(|c| (c.id, vec![g])));
    }
    if let Some(gains) = meta.values.get("ColourGains").filter(|g| g.len() == 2) {
        out.extend(find("AwbEnable").map(|c| (c.id, vec![0.0])));
        out.extend(find("ColourGains").map(|c| (c.id, gains.clone())));
    }
    if let Some(lens) = meta.get("LensPosition") {
        out.extend(manual("AfMode"));
        out.extend(find("LensPosition").map(|c| (c.id, vec![lens])));
    }
    out
}

fn acquire(cameras: &libcamera::camera_manager::CameraList<'static>, index: usize) -> Result<ActiveCamera<'static>, String> {
    let cam = cameras.get(index).ok_or_else(|| format!("no camera {index}"))?;
    let active = cam.acquire().map_err(|e| format!("cannot acquire camera: {e}"))?;
    perf!("camera-acquired", "camera={index}");
    Ok(active)
}

/// Whether a Still session's frame shows the held exposure and gain (within
/// 10%). With nothing held, the first frame will do.
// ponytail: eight frames is the patience for a held exposure to show up; a
// camera that ignores manual controls still gets its photo.
fn settled(expect: (Option<f64>, Option<f64>), exposure: Option<f64>, gain: Option<f64>, frames: u32) -> bool {
    let near = |want: Option<f64>, got: Option<f64>| match (want, got) {
        (Some(w), Some(g)) => (g - w).abs() <= w.abs() * 0.1 + 1e-3,
        (Some(_), None) => false,
        _ => frames >= 1,
    };
    frames >= 8 || (near(expect.0, exposure) && near(expect.1, gain))
}

fn control_value(desc: &ControlDesc, value: &[f64]) -> Option<ControlValue> {
    Some(match desc.kind {
        Kind::Bool => ControlValue::Bool(value.iter().map(|x| *x != 0.0).collect()),
        Kind::Int if control_type(desc.id) == Some(LIBCAMERA_INT64) => ControlValue::Int64(value.iter().map(|x| x.round() as i64).collect()),
        Kind::Int => ControlValue::Int32(value.iter().map(|x| x.round() as i32).collect()),
        Kind::Float => ControlValue::Float(value.iter().map(|x| *x as f32).collect()),
        Kind::Other => return None,
    })
}

impl Live {
    fn open(
        mut cam: ActiveCamera<'static>,
        index: usize,
        mut info: CameraInfo,
        mode: Option<Mode>,
        purpose: Purpose,
        initial: &[(u32, Vec<f64>)],
        tx: &Sender<Internal>,
    ) -> Result<(Self, Session), String> {
        let modes = sensor_modes(&cam);
        let photo = mode.filter(|m| modes.contains(m)).or_else(|| modes.first().copied());
        let view_mode = match (purpose, photo) {
            (Purpose::Preview, Some(p)) => Some(viewfinder_mode(&modes, p)),
            _ => photo,
        };
        // A raw stream where a photo can come from this session; a preview
        // that hands photos to a Still session does not need one.
        let want_raw = purpose != Purpose::Preview || view_mode == photo;
        let (mut cfg, raw) = match view_mode.filter(|_| want_raw).and_then(|m| configure(&cam, m, true)) {
            Some(cfg) => (cfg, true),
            None => (
                configure(&cam, view_mode.unwrap_or(Mode { width: 1280, height: 720 }), false)
                    .ok_or("no usable camera configuration")?,
                false,
            ),
        };
        cam.configure(&mut cfg).map_err(|e| format!("configure: {e}"))?;
        perf!("camera-configured", "{}", cfg.get(0).map(|c| c.to_string_repr()).unwrap_or_default());
        // The orientation validate() settled on is what the buffers really
        // hold: the mounting rotation minus whatever sensor flips could undo.
        // properties::Rotation alone would double-correct a flipped sensor.
        info.rotation = upright_rotation(cfg.orientation());

        let view_cfg = cfg.get(0).unwrap();
        let view = view_cfg.stream().ok_or("no viewfinder stream")?;
        let view_size = view_cfg.get_size();
        let raw_stream = if raw { cfg.get(1).and_then(|c| c.stream()) } else { None };
        let view_mode = match raw_stream.and_then(|s| s.configuration().map(|c| c.get_size())) {
            Some(size) => Mode { width: size.width, height: size.height },
            None => Mode { width: view_size.width, height: view_size.height },
        };
        let mode = match photo {
            Some(p) if !want_raw => p,
            _ => view_mode,
        };

        let mut alloc = FrameBufferAllocator::new(&cam);
        let view_bufs = alloc.alloc(&view).map_err(|e| format!("alloc: {e}"))?;
        let raw_bufs = match raw_stream {
            Some(s) => alloc.alloc(&s).map_err(|e| format!("alloc raw: {e}"))?,
            None => Vec::new(),
        };

        let mut requests = Vec::new();
        let mut raw_bufs = raw_bufs.into_iter();
        for (i, buf) in view_bufs.into_iter().enumerate() {
            let mut req = cam.create_request(Some(i as u64)).ok_or("create request")?;
            let buf = MemoryMappedFrameBuffer::new(buf).map_err(|e| format!("mmap: {e:?}"))?;
            req.add_buffer(&view, buf).map_err(|e| format!("add buffer: {e}"))?;
            if let (Some(s), Some(rb)) = (raw_stream, raw_bufs.next()) {
                let rb = MemoryMappedFrameBuffer::new(rb).map_err(|e| format!("mmap raw: {e:?}"))?;
                req.add_buffer(&s, rb).map_err(|e| format!("add raw buffer: {e}"))?;
            }
            requests.push(req);
        }

        let done = tx.clone();
        cam.on_request_completed(move |req| {
            let _ = done.send(Internal::Done(req));
        });
        let controls = describe_controls(&cam);
        let mut start = ControlList::new();
        for (id, value) in initial {
            if let Some(v) = controls.iter().find(|c| c.id == *id).and_then(|d| control_value(d, value)) {
                let _ = start.set_raw(*id, v);
            }
        }
        cam.start(Some(&start)).map_err(|e| format!("start: {e}"))?;
        let outstanding = requests.len();
        for req in requests {
            cam.queue_request(req).map_err(|(_, e)| format!("queue: {e}"))?;
        }

        perf!("camera-started", "requests={outstanding} purpose={purpose:?} view={}x{}", view_size.width, view_size.height);
        let fps = frame_duration_range(&cam);
        let crop_max = cam.properties().get::<properties::ScalerCropMaximum>().ok().map(|r| r.0);
        let af_windows = crop_max.is_some() && ["AfWindows", "AfMetering"].iter().all(|n| controls.iter().any(|c| c.name == *n));
        let session = Session {
            camera: index,
            info,
            modes,
            mode,
            view: Mode { width: view_size.width, height: view_size.height },
            fourcc: view.configuration().map(|c| c.get_pixel_format().fourcc()).unwrap_or(0),
            controls,
            // A preview that passes photos to a Still session can save RAW
            // when the camera has a raw role at all.
            raw: raw_stream.is_some() || (!want_raw && sensor_modes_are_raw(&cam)),
            fps,
            af_windows,
        };
        let pending = ControlList::new();
        Ok((
            Live {
                cam,
                _alloc: alloc,
                view,
                raw: raw_stream,
                info: session.clone(),
                pending,
                capture_next: false,
                recorder: None,
                generation: {
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                },
                first_frame: true,
                last_meta: Instant::now(),
                outstanding,
                crop_max,
                purpose,
                view_mode,
                user: initial.to_vec(),
                meta: Metadata::default(),
                expect: (None, None),
                frames_seen: 0,
            },
            session,
        ))
    }

    fn set_control(&mut self, id: u32, value: &[f64]) {
        let Some(desc) = self.info.controls.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(v) = control_value(desc, value) else { return };
        perf!("control", "{}={value:?}", desc.name);
        // Triggers are one-shot; everything else is a setting to carry over.
        if desc.name != "AfTrigger" {
            self.user.retain(|(i, _)| *i != id);
            self.user.push((id, value.to_vec()));
        }
        if let Err(e) = self.pending.set_raw(id, v) {
            log::warn!("set {}: {e}", desc.name);
        }
    }

    fn focus_at(&mut self, x: f64, y: f64) {
        let Some(max) = self.crop_max else { return };
        let find = |name: &str| self.info.controls.iter().find(|c| c.name == name);
        let (Some(windows), Some(metering)) = (find("AfWindows"), find("AfMetering")) else { return };
        let Some((mode, _)) = metering.enums.iter().find(|(_, n)| n.ends_with("Windows")) else { return };
        // An eighth of the frame each way, centred on the point, kept inside.
        let (w, h) = (max.width / 8, max.height / 8);
        let cx = (x.clamp(0.0, 1.0) * max.width as f64) as i64 - w as i64 / 2;
        let cy = (y.clamp(0.0, 1.0) * max.height as f64) as i64 - h as i64 / 2;
        let rect = libcamera::geometry::Rectangle {
            x: max.x + cx.clamp(0, (max.width - w) as i64) as i32,
            y: max.y + cy.clamp(0, (max.height - h) as i64) as i32,
            width: w,
            height: h,
        };
        let (windows, metering, mode) = (windows.id, metering.id, *mode);
        let _ = self.pending.set_raw(metering, ControlValue::Int32(smallvec::smallvec![mode]));
        if let Err(e) = self.pending.set_raw(windows, ControlValue::Rectangle(smallvec::smallvec![rect])) {
            log::warn!("set AfWindows: {e}");
        }
    }

    fn completed(&mut self, req: Request, copy: bool, tx: &Sender<Internal>, slot: &FrameSlot, emit: &impl Fn(Event)) {
        use libcamera::request::RequestStatus;
        if req.status() != RequestStatus::Complete {
            self.outstanding -= 1;
            return drop(req);
        }

        if self.purpose == Purpose::Still {
            self.frames_seen += 1;
            let meta = read_metadata(req.metadata());
            if !settled(self.expect, meta.get("ExposureTime"), meta.get("AnalogueGain"), self.frames_seen) {
                return self.requeue(req);
            }
            perf!("still-settled", "frames={}", self.frames_seen);
        } else if self.last_meta.elapsed() > Duration::from_millis(200) {
            self.last_meta = Instant::now();
            self.meta = read_metadata(req.metadata());
            emit(Event::Metadata(self.meta.clone()));
        }

        let Some((width, height, stride, fourcc)) = self
            .view
            .configuration()
            .map(|c| (c.get_size().width, c.get_size().height, c.get_stride(), c.get_pixel_format().fourcc()))
        else {
            return self.requeue(req);
        };
        let Some((fd, offset, bytes, still)) = self.inspect(&req, copy, width, height, stride, fourcc) else {
            return self.requeue(req);
        };
        if let Some(still) = still {
            emit(Event::Still(still));
        }
        if std::mem::take(&mut self.first_frame) {
            perf!("frame-first-queued");
        }
        // A Still session's frames are never shown: the interface holds the
        // last viewfinder picture meanwhile.
        if self.purpose == Purpose::Still {
            return self.requeue(req);
        }
        if copy {
            slot.deliver(Frame { width, height, stride, fourcc, fd, offset, bytes, ret: None }, emit);
            return self.requeue(req);
        }
        slot.deliver(Frame { width, height, stride, fourcc, fd, offset, bytes: None, ret: Some((req, tx.clone(), self.generation)) }, emit);
    }

    /// Everything a completed request's buffers are needed for, read while
    /// the request is still borrowed.
    #[allow(clippy::type_complexity)]
    fn inspect(
        &mut self,
        req: &Request,
        copy: bool,
        width: u32,
        height: u32,
        stride: u32,
        fourcc: u32,
    ) -> Option<(RawFd, u32, Option<Vec<u8>>, Option<Box<Still>>)> {
        let buf = req.buffer::<Buffer>(&self.view)?;
        let planes = buf.planes();
        let plane = planes.get(0)?;
        let (fd, offset) = (plane.fd(), plane.offset().unwrap_or(0) as u32);
        let data = buf.data();
        let view = data.first()?;
        let bytes = copy.then(|| view.to_vec());
        if let Some(r) = &self.recorder {
            r.push(view, stride);
        }

        let still = std::mem::take(&mut self.capture_next).then(|| {
            let copy_started = Instant::now();
            let raw = self.raw.and_then(|s| {
                let rb = req.buffer::<Buffer>(&s)?;
                let c = s.configuration()?;
                let used = rb.metadata().and_then(|m| m.planes().get(0).map(|p| p.bytes_used as usize)).unwrap_or(0);
                let planes = rb.data();
                let data = planes.first()?;
                let len = if used > 0 { used.min(data.len()) } else { data.len() };
                Some(RawImage {
                    width: c.get_size().width,
                    height: c.get_size().height,
                    stride: c.get_stride(),
                    format: c.get_pixel_format().to_string(),
                    data: data[..len].to_vec(),
                })
            });
            let still = Box::new(Still {
                width,
                height,
                stride,
                fourcc,
                rgba: view.to_vec(),
                raw,
                metadata: read_metadata(req.metadata()),
                info: self.info.info.clone(),
                zoom: 1.0,
            });
            perf!("still-copied", "ms={:.1} bytes={}", copy_started.elapsed().as_secs_f64() * 1e3, still.rgba.len());
            still
        });
        Some((fd, offset, bytes, still))
    }

    fn requeue(&mut self, mut req: Request) {
        req.reuse(ReuseFlag::REUSE_BUFFERS);
        if !self.pending.is_empty() {
            req.controls_mut().merge(&self.pending, libcamera::control::MergePolicy::OverwriteExisting);
            self.pending.clear();
        }
        if let Err((req, e)) = self.cam.queue_request(req) {
            log::warn!("requeue: {e}");
            self.outstanding -= 1;
            drop(req);
        }
    }

    /// Stop, then wait for the requests the UI still holds so no buffer is
    /// freed under a texture.
    /// Returns the commands that arrived meanwhile.
    /// Stop, take back every request, free the buffers; the camera stays
    /// acquired for whoever opens next. Also returns the commands that
    /// arrived meanwhile.
    fn close(mut self, rx: &Receiver<Internal>, slot: &FrameSlot) -> (ActiveCamera<'static>, Vec<Internal>) {
        let _ = self.cam.stop();
        // A frame waiting in the slot is one of ours.
        drop(slot.frame.lock().unwrap().take());
        let mut deferred = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(800);
        while self.outstanding > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Internal::Done(req)) | Ok(Internal::Returned(req, _)) => {
                    self.outstanding -= 1;
                    drop(req);
                }
                Ok(cmd @ Internal::Cmd(_)) => deferred.push(cmd),
                Err(_) => {
                    perf!("close-timeout", "outstanding={} generation={}", self.outstanding, self.generation);
                    log::warn!("{} requests still out at close", self.outstanding);
                    break;
                }
            }
        }
        let Live { cam, _alloc, .. } = self;
        drop(_alloc);
        (cam, deferred)
    }
}

/// Distinct sensor output sizes, largest first, from the raw role's formats.
fn sensor_modes_are_raw(cam: &ActiveCamera) -> bool {
    cam.generate_configuration(&[StreamRole::Raw]).is_some_and(|c| c.get(0).is_some())
}

fn sensor_modes(cam: &ActiveCamera) -> Vec<Mode> {
    let mut modes = Vec::new();
    for role in [StreamRole::Raw, StreamRole::ViewFinder] {
        let Some(cfg) = cam.generate_configuration(&[role]) else { continue };
        let Some(sc) = cfg.get(0) else { continue };
        let formats = sc.formats();
        for pf in formats.pixel_formats().into_iter() {
            // A range lists common sizes inside it, not what the camera can
            // do: add its largest, and later keep only sizes that configure
            // as asked.
            let max = formats.range(pf).max;
            // With the largest, its half (a binned mode) and 16:9 of both.
            let derived = (max.width > 0).then(|| {
                let half = Size { width: (max.width / 2) & !1, height: (max.height / 2) & !1 };
                let wide = |s: Size| Size { width: s.width, height: (s.width * 9 / 16) & !1 };
                [max, half, wide(max), wide(half)]
            });
            for s in formats.sizes(pf).into_iter().chain(derived.into_iter().flatten()) {
                let m = Mode { width: s.width, height: s.height };
                if !modes.contains(&m) {
                    modes.push(m);
                }
            }
        }
        if matches!(role, StreamRole::ViewFinder) {
            modes.retain(|m| configures_as(cam, *m));
        }
        if !modes.is_empty() {
            break;
        }
    }
    modes.sort_by_key(|m| std::cmp::Reverse(m.width as u64 * m.height as u64));
    modes
}

/// Whether a viewfinder stream of this size validates without being resized.
fn configures_as(cam: &ActiveCamera, mode: Mode) -> bool {
    let Some(mut cfg) = cam.generate_configuration(&[StreamRole::ViewFinder]) else { return false };
    if let Some(mut s) = cfg.get_mut(0) {
        s.set_size(Size { width: mode.width, height: mode.height });
    }
    let valid = !matches!(cfg.validate(), CameraConfigurationStatus::Invalid);
    valid && cfg.get(0).is_some_and(|s| (s.get_size().width, s.get_size().height) == (mode.width, mode.height))
}

fn configure(cam: &ActiveCamera, mode: Mode, raw: bool) -> Option<CameraConfiguration> {
    let roles: &[StreamRole] = if raw { &[StreamRole::ViewFinder, StreamRole::Raw] } else { &[StreamRole::ViewFinder] };
    let mut cfg = cam.generate_configuration(roles)?;
    {
        let mut vf = cfg.get_mut(0)?;
        let offered: Vec<PixelFormat> = vf.formats().pixel_formats().into_iter().collect();
        if let Some(pf) = VIEW_FORMATS
            .iter()
            .map(|c| fourcc(c))
            .find_map(|code| offered.iter().copied().find(|pf| pf.fourcc() == code && pf.modifier() == 0))
        {
            vf.set_pixel_format(pf);
        }
        vf.set_size(Size { width: mode.width, height: mode.height });
    }
    if raw {
        let mut r = cfg.get_mut(1)?;
        r.set_size(Size { width: mode.width, height: mode.height });
    }
    match cfg.validate() {
        CameraConfigurationStatus::Invalid => None,
        _ => {
            if raw {
                let r = cfg.get(1)?;
                let got = r.get_size();
                if (got.width, got.height) != (mode.width, mode.height) {
                    return None;
                }
            }
            Some(cfg)
        }
    }
}

/// libcamera orientations name the clockwise turn that produced the buffer
/// from an upright image; undoing it is the opposite turn.
// ponytail: mirrored orientations only come back when an app asks for one,
// and Obscura never does, so the mirror bit is dropped.
fn upright_rotation(o: Orientation) -> i32 {
    match o {
        Orientation::Rotate0 | Orientation::Rotate0Mirror => 0,
        Orientation::Rotate90 | Orientation::Rotate90Mirror => 270,
        Orientation::Rotate180 | Orientation::Rotate180Mirror => 180,
        Orientation::Rotate270 | Orientation::Rotate270Mirror => 90,
    }
}

fn frame_duration_range(cam: &ActiveCamera) -> Option<(f64, f64)> {
    let id = id_by_name(cam, "FrameDurationLimits")?;
    let info = cam.controls().find(id).ok()?;
    let (min, max) = (numbers(&info.min()), numbers(&info.max()));
    // Durations are microseconds; fps is the inverse, so min duration = max fps.
    let fastest = *min.first()?;
    let slowest = *max.last().or(max.first())?;
    (fastest > 0.0 && slowest > 0.0).then(|| (1e6 / slowest, 1e6 / fastest))
}

fn id_by_name(cam: &ActiveCamera, name: &str) -> Option<u32> {
    cam.controls().into_iter().map(|(id, _)| id).find(|id| control_name(*id).as_deref() == Some(name))
}

const LIBCAMERA_INT64: u32 = 4;

fn control_ptr(id: u32) -> *const libcamera_sys::libcamera_control_id_t {
    unsafe { libcamera_sys::libcamera_control_from_id(id as _) }
}

pub fn control_name(id: u32) -> Option<String> {
    let p = unsafe { libcamera_sys::libcamera_control_name_from_id(id as _) };
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn control_type(id: u32) -> Option<u32> {
    let p = control_ptr(id);
    (!p.is_null()).then(|| unsafe { libcamera_sys::libcamera_control_id_type(p.cast_mut()) } as u32)
}

fn describe_controls(cam: &ActiveCamera) -> Vec<ControlDesc> {
    let mut out = Vec::new();
    for (id, info) in cam.controls() {
        let p = control_ptr(id);
        if p.is_null() {
            continue;
        }
        let name = control_name(id).unwrap_or_else(|| format!("Control {id}"));
        let (is_array, size) = unsafe {
            (
                libcamera_sys::libcamera_control_id_is_array(p.cast_mut()),
                libcamera_sys::libcamera_control_id_size(p.cast_mut()),
            )
        };
        let min = numbers(&info.min());
        let max = numbers(&info.max());
        let def = numbers(&info.def());
        let kind = match info.min() {
            ControlValue::Bool(_) => Kind::Bool,
            ControlValue::Byte(_) | ControlValue::Uint16(_) | ControlValue::Uint32(_) | ControlValue::Int32(_) | ControlValue::Int64(_) => Kind::Int,
            ControlValue::Float(_) => Kind::Float,
            _ => Kind::Other,
        };
        let enums = enumerators(p)
            .into_iter()
            .filter(|(v, _)| {
                let allowed = info.values();
                allowed.is_empty() || allowed.iter().any(|a| numbers(a).first() == Some(&(*v as f64)))
            })
            .collect();
        out.push(ControlDesc {
            id,
            name,
            kind,
            len: if is_array { size } else { 1 },
            min: min.first().copied().unwrap_or(0.0),
            max: max.first().copied().unwrap_or(0.0),
            def,
            enums,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn enumerators(p: *const libcamera_sys::libcamera_control_id_t) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    unsafe {
        let it = libcamera_sys::libcamera_control_id_enumerators_iter_create(p.cast_mut());
        if it.is_null() {
            return out;
        }
        while libcamera_sys::libcamera_control_id_enumerators_iter_has_next(it) {
            let key = libcamera_sys::libcamera_control_id_enumerators_iter_key(it);
            let val = libcamera_sys::libcamera_control_id_enumerators_iter_value(it);
            if !val.is_null() {
                out.push((key, CStr::from_ptr(val).to_string_lossy().into_owned()));
            }
            libcamera_sys::libcamera_control_id_enumerators_iter_next(it);
        }
        libcamera_sys::libcamera_control_id_enumerators_iter_destroy(it);
    }
    out
}

pub fn numbers(v: &ControlValue) -> Vec<f64> {
    fn f<T: Copy + Into<f64>, const N: usize>(s: &SmallVec<[T; N]>) -> Vec<f64> where [T; N]: smallvec::Array<Item = T> {
        s.iter().map(|x| (*x).into()).collect()
    }
    match v {
        ControlValue::Bool(s) => s.iter().map(|b| *b as u8 as f64).collect(),
        ControlValue::Byte(s) => f(s),
        ControlValue::Uint16(s) => f(s),
        ControlValue::Uint32(s) => f(s),
        ControlValue::Int32(s) => f(s),
        ControlValue::Int64(s) => s.iter().map(|x| *x as f64).collect(),
        ControlValue::Float(s) => f(s),
        _ => Vec::new(),
    }
}

fn read_metadata(list: &ControlList) -> Metadata {
    let mut values = HashMap::new();
    for (id, value) in list {
        if let Some(name) = control_name(id) {
            let n = numbers(&value);
            if !n.is_empty() {
                values.insert(name, n);
            }
        }
    }
    Metadata { values }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_waits_for_the_held_exposure() {
        let held = (Some(20000.0), Some(4.0));
        assert!(!settled(held, Some(33333.0), Some(4.0), 1));
        assert!(settled(held, Some(20500.0), Some(4.1), 2));
        assert!(!settled(held, None, None, 5));
        assert!(settled(held, Some(33333.0), Some(1.0), 8));
        assert!(settled((None, None), None, None, 1));
    }

    #[test]
    fn seed_holds_what_auto_chose() {
        let d = |id, name: &str, enums: &[&str]| ControlDesc {
            id,
            name: name.into(),
            kind: Kind::Int,
            len: 1,
            min: 0.0,
            max: 1.0,
            def: vec![0.0],
            enums: enums.iter().enumerate().map(|(i, n)| (i as i32, n.to_string())).collect(),
        };
        let controls = [d(1, "ExposureTimeMode", &["ExposureTimeModeAuto", "ExposureTimeModeManual"]), d(2, "ExposureTime", &[]), d(3, "AnalogueGain", &[]), d(4, "AfMode", &["AfModeManual", "AfModeAuto", "AfModeContinuous"]), d(5, "LensPosition", &[])];
        let mut meta = Metadata::default();
        for (k, v) in [("ExposureTime", 16666.0), ("AnalogueGain", 2.0), ("LensPosition", 1.5)] {
            meta.values.insert(k.into(), vec![v]);
        }
        let seeded = seed(&controls, &meta);
        assert_eq!(seeded, vec![(1, vec![1.0]), (2, vec![16666.0]), (3, vec![2.0]), (4, vec![0.0]), (5, vec![1.5])]);
    }

    #[test]
    fn viewfinder_takes_the_binned_mode_of_the_same_shape() {
        let m = |width, height| Mode { width, height };
        let imx362 = [m(4032, 3032), m(4032, 2272), m(2016, 1512), m(2016, 1136)];
        assert_eq!(viewfinder_mode(&imx362, m(4032, 3032)), m(2016, 1512));
        assert_eq!(viewfinder_mode(&imx362, m(4032, 2272)), m(2016, 1136));
        assert_eq!(viewfinder_mode(&imx362, m(2016, 1512)), m(2016, 1512));
        // A webcam's largest mode is also its viewfinder.
        assert_eq!(viewfinder_mode(&[m(1920, 1080), m(1280, 720)], m(1920, 1080)), m(1920, 1080));
    }

    #[test]
    fn rotation_undoes_the_orientation() {
        // Pixel 2 XL: Rotation 90, no transpose-capable sensor, so the
        // buffers are Rotate90 and need a quarter turn counter-clockwise.
        assert_eq!(upright_rotation(Orientation::Rotate90), 270);
        assert_eq!(upright_rotation(Orientation::Rotate270), 90);
        assert_eq!(upright_rotation(Orientation::Rotate0), 0);
    }
}
