// SPDX-License-Identifier: GPL-3.0-or-later
//! Capture backends. The UI talks to a backend only through [`Cmd`] and
//! [`Event`], so a PipeWire backend (for a sandboxed build, which gets a
//! PipeWire remote from the camera portal instead of device access) can sit
//! next to the libcamera one without the UI knowing which it has.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::fd::RawFd;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use libcamera::camera::{ActiveCamera, CameraConfiguration, CameraConfigurationStatus};
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
    /// Degrees the image must be rotated clockwise to be upright.
    pub rotation: i32,
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
    pub controls: Vec<ControlDesc>,
    pub raw: bool,
    /// Frame rate range the configured mode allows, if the camera reports it.
    pub fps: Option<(f64, f64)>,
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
    pub size: usize,
    /// CPU copy, only when the backend was told the dmabuf path failed.
    pub bytes: Option<Vec<u8>>,
    ret: Option<(Request, Sender<Internal>)>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{} {:08x})", self.width, self.height, self.fourcc)
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        if let Some((req, tx)) = self.ret.take() {
            let _ = tx.send(Internal::Returned(req));
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
}

#[derive(Debug)]
pub enum Cmd {
    Open { camera: usize, mode: Option<Mode> },
    SetControl { id: u32, value: Vec<f64> },
    Capture,
    CopyFrames(bool),
    Close,
}

#[derive(Debug)]
pub enum Event {
    Cameras(Vec<CameraInfo>),
    Opened(Session),
    Frame(Frame),
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
    Returned(Request),
}

pub struct LibcameraBackend {
    tx: Sender<Internal>,
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
        std::thread::Builder::new()
            .name("libcamera".into())
            .spawn(move || run(rx, worker_tx, emit))
            .expect("spawn libcamera thread");
        Self { tx }
    }
}

/// Viewfinder formats in order of preference. libcamera pixel formats are DRM
/// fourccs, which is what GdkDmabufTextureBuilder wants too.
const VIEW_FORMATS: &[&[u8; 4]] = &[b"XB24", b"AB24", b"XR24", b"AR24", b"NV12", b"YUYV"];

fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

fn run(rx: Receiver<Internal>, tx: Sender<Internal>, emit: impl Fn(Event)) {
    let mgr = match CameraManager::new() {
        Ok(mgr) => mgr,
        Err(e) => return emit(Event::Error(format!("libcamera: {e}"))),
    };
    let cameras = mgr.cameras();
    let infos: Vec<CameraInfo> = (0..cameras.len())
        .filter_map(|i| cameras.get(i))
        .map(|cam| {
            let props = cam.properties();
            CameraInfo {
                id: cam.id().to_string(),
                model: props
                    .get::<properties::Model>()
                    .map(|m| m.0)
                    .unwrap_or_else(|_| cam.id().to_string()),
                facing: match props.get::<properties::Location>() {
                    Ok(properties::Location::CameraFront) => Facing::Front,
                    Ok(properties::Location::CameraBack) => Facing::Back,
                    _ => Facing::External,
                },
                rotation: props.get::<properties::Rotation>().map(|r| r.0).unwrap_or(0),
            }
        })
        .collect();
    emit(Event::Cameras(infos.clone()));

    let mut session: Option<Live> = None;
    let mut copy_frames = false;

    while let Ok(msg) = rx.recv() {
        match msg {
            Internal::Cmd(Cmd::Open { camera, mode }) => {
                if let Some(live) = session.take() {
                    live.close(&rx);
                }
                let Some(cam) = cameras.get(camera) else {
                    emit(Event::Error(format!("no camera {camera}")));
                    continue;
                };
                match Live::open(cam, camera, infos[camera].clone(), mode, &tx) {
                    Ok((live, info)) => {
                        emit(Event::Opened(info));
                        session = Some(live);
                    }
                    Err(e) => emit(Event::Error(e)),
                }
            }
            Internal::Cmd(Cmd::SetControl { id, value }) => {
                if let Some(live) = session.as_mut() {
                    live.set_control(id, &value);
                }
            }
            Internal::Cmd(Cmd::Capture) => {
                if let Some(live) = session.as_mut() {
                    live.capture_next = true;
                }
            }
            Internal::Cmd(Cmd::CopyFrames(on)) => copy_frames = on,
            Internal::Cmd(Cmd::Close) => {
                if let Some(live) = session.take() {
                    live.close(&rx);
                }
            }
            Internal::Done(req) => match session.as_mut() {
                Some(live) => live.completed(req, copy_frames, &tx, &emit),
                None => drop(req),
            },
            Internal::Returned(req) => match session.as_mut() {
                Some(live) => live.requeue(req),
                None => drop(req),
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
    last_meta: Instant,
    outstanding: usize,
}

impl Live {
    fn open(
        cam: libcamera::camera::Camera<'static>,
        index: usize,
        info: CameraInfo,
        mode: Option<Mode>,
        tx: &Sender<Internal>,
    ) -> Result<(Self, Session), String> {
        let mut cam = cam.acquire().map_err(|e| format!("cannot acquire camera: {e}"))?;
        let modes = sensor_modes(&cam);
        let mode = mode
            .filter(|m| modes.contains(m))
            .or_else(|| modes.first().copied());

        // Viewfinder plus a raw stream when the pipeline can do both, else
        // the viewfinder alone.
        let (mut cfg, raw) = match mode.and_then(|m| configure(&cam, m, true)) {
            Some(cfg) => (cfg, true),
            None => (
                configure(&cam, mode.unwrap_or(Mode { width: 1280, height: 720 }), false)
                    .ok_or("no usable camera configuration")?,
                false,
            ),
        };
        cam.configure(&mut cfg).map_err(|e| format!("configure: {e}"))?;

        let view_cfg = cfg.get(0).unwrap();
        let view = view_cfg.stream().ok_or("no viewfinder stream")?;
        let view_size = view_cfg.get_size();
        let raw_stream = if raw { cfg.get(1).and_then(|c| c.stream()) } else { None };
        let mode = match raw_stream.and_then(|s| s.configuration().map(|c| c.get_size())) {
            Some(size) => Mode { width: size.width, height: size.height },
            None => Mode { width: view_size.width, height: view_size.height },
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
        cam.start(None).map_err(|e| format!("start: {e}"))?;
        let outstanding = requests.len();
        for req in requests {
            cam.queue_request(req).map_err(|(_, e)| format!("queue: {e}"))?;
        }

        let controls = describe_controls(&cam);
        let fps = frame_duration_range(&cam);
        let session = Session {
            camera: index,
            info,
            modes,
            mode,
            view: Mode { width: view_size.width, height: view_size.height },
            controls,
            raw: raw_stream.is_some(),
            fps,
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
                last_meta: Instant::now(),
                outstanding,
            },
            session,
        ))
    }

    fn set_control(&mut self, id: u32, value: &[f64]) {
        let Some(desc) = self.info.controls.iter().find(|c| c.id == id) else {
            return;
        };
        let v = match desc.kind {
            Kind::Bool => ControlValue::Bool(value.iter().map(|x| *x != 0.0).collect()),
            Kind::Int if control_type(id) == Some(LIBCAMERA_INT64) => {
                ControlValue::Int64(value.iter().map(|x| x.round() as i64).collect())
            }
            Kind::Int => ControlValue::Int32(value.iter().map(|x| x.round() as i32).collect()),
            Kind::Float => ControlValue::Float(value.iter().map(|x| *x as f32).collect()),
            Kind::Other => return,
        };
        if let Err(e) = self.pending.set_raw(id, v) {
            log::warn!("set {}: {e}", desc.name);
        }
    }

    fn completed(&mut self, req: Request, copy: bool, tx: &Sender<Internal>, emit: &impl Fn(Event)) {
        use libcamera::request::RequestStatus;
        if req.status() != RequestStatus::Complete {
            self.outstanding -= 1;
            return drop(req);
        }

        if self.last_meta.elapsed() > Duration::from_millis(200) {
            self.last_meta = Instant::now();
            emit(Event::Metadata(read_metadata(req.metadata())));
        }

        let Some((width, height, stride, fourcc)) = self
            .view
            .configuration()
            .map(|c| (c.get_size().width, c.get_size().height, c.get_stride(), c.get_pixel_format().fourcc()))
        else {
            return self.requeue(req);
        };
        let Some((fd, offset, size, bytes, still)) = self.inspect(&req, copy, width, height, stride, fourcc) else {
            return self.requeue(req);
        };
        if let Some(still) = still {
            emit(Event::Still(still));
        }
        if copy {
            emit(Event::Frame(Frame { width, height, stride, fourcc, fd, offset, size, bytes, ret: None }));
            return self.requeue(req);
        }
        emit(Event::Frame(Frame { width, height, stride, fourcc, fd, offset, size, bytes: None, ret: Some((req, tx.clone())) }));
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
    ) -> Option<(RawFd, u32, usize, Option<Vec<u8>>, Option<Box<Still>>)> {
        let buf = req.buffer::<Buffer>(&self.view)?;
        let planes = buf.planes();
        let plane = planes.get(0)?;
        let (fd, offset, size) = (plane.fd(), plane.offset().unwrap_or(0) as u32, plane.len());
        let data = buf.data();
        let view = data.first()?;
        let bytes = copy.then(|| view.to_vec());

        let still = std::mem::take(&mut self.capture_next).then(|| {
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
            Box::new(Still {
                width,
                height,
                stride,
                fourcc,
                rgba: view.to_vec(),
                raw,
                metadata: read_metadata(req.metadata()),
                info: self.info.info.clone(),
            })
        });
        Some((fd, offset, size, bytes, still))
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
    fn close(mut self, rx: &Receiver<Internal>) {
        let _ = self.cam.stop();
        let deadline = Instant::now() + Duration::from_millis(800);
        while self.outstanding > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Internal::Done(req)) | Ok(Internal::Returned(req)) => {
                    self.outstanding -= 1;
                    drop(req);
                }
                Ok(Internal::Cmd(_)) => {}
                Err(_) => {
                    log::warn!("{} requests still out at close", self.outstanding);
                    break;
                }
            }
        }
    }
}

/// Distinct sensor output sizes, largest first, from the raw role's formats.
fn sensor_modes(cam: &ActiveCamera) -> Vec<Mode> {
    let mut modes = Vec::new();
    for role in [StreamRole::Raw, StreamRole::ViewFinder] {
        let Some(cfg) = cam.generate_configuration(&[role]) else { continue };
        let Some(sc) = cfg.get(0) else { continue };
        let formats = sc.formats();
        for pf in formats.pixel_formats().into_iter() {
            for s in formats.sizes(pf) {
                let m = Mode { width: s.width, height: s.height };
                if !modes.contains(&m) {
                    modes.push(m);
                }
            }
        }
        if !modes.is_empty() {
            break;
        }
    }
    modes.sort_by_key(|m| std::cmp::Reverse(m.width as u64 * m.height as u64));
    modes
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

