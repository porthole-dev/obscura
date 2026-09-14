// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gettextrs::gettext;
use relm4::adw::{self, prelude::*};
use relm4::gtk::{self, gdk, gio, glib};
use relm4::{Component, ComponentParts, ComponentSender};

use crate::camera::{Backend, CameraInfo, Cmd, Event, Facing, LibcameraBackend, Metadata, Mode, Session, Still};
use crate::controls::{self, Panel, format_value};
use crate::portal::{self, Access};
use crate::video::Recorder;
use crate::viewfinder::{self, Viewfinder};
use crate::APP_ID;

pub struct App {
    backend: Option<Rc<LibcameraBackend>>,
    cameras: Vec<CameraInfo>,
    /// The camera open, or being opened.
    camera: usize,
    session: Option<Session>,
    panel: Option<Rc<Panel>>,
    copy_frames: bool,
    /// Fade the next frame in: the camera or mode just changed.
    awaiting_frame: bool,
    frames: u32,
    fps_since: Instant,
    fps: f64,
    last_capture: Option<PathBuf>,
    settings: Option<gio::Settings>,
    saving: bool,
    video: bool,
    recorder: Option<Arc<Recorder>>,
    record_started: Option<Instant>,
    record_tick: Option<glib::SourceId>,
    frame_duration: Option<f64>,
    /// Self-timer seconds, 0 for off.
    timer: u32,
    countdown: Option<(u32, glib::SourceId)>,
    /// Bumped per tap, so a late hide does not hide a newer focus ring.
    focus_generation: u32,
    focusing: bool,
    /// The compositor says nobody can see the window: the camera is closed.
    suspended: bool,
    perf: Option<Rc<std::cell::RefCell<PerfStats>>>,
    /// The portal said yes (or is not there): cameras may be opened.
    granted: bool,
    /// The photo path has been warmed up in the background.
    warmed: bool,
    /// Open the last capture as soon as it is written.
    open_pending: bool,
    last_meta: Metadata,
    /// AE/AF lock is on; with the exposure time it locked at, if it did.
    locked: bool,
    lock_exposure: Option<f64>,
    /// Device turn from the accelerometer, and the display turn it implies.
    device: i32,
    natural_landscape: Option<bool>,
    display_rotation: i32,
    zoom: f64,
    zoom_start: f64,
    orientation: crate::device::Orientation,
}

/// Viewfinder counters for OBSCURA_PERF, shared with the frame clock.
#[derive(Default)]
struct PerfStats {
    delivered: u32,
    presented: u32,
    dropped: u32,
    set_since_paint: u32,
    main_ns: u64,
    main_max_ns: u64,
    since: Option<Instant>,
    /// What the next presented frame completes: an open, a switch.
    awaiting: Option<String>,
    shutter: Option<Instant>,
}

impl PerfStats {
    fn painted(&mut self) {
        if self.set_since_paint == 0 {
            return;
        }
        self.presented += 1;
        self.dropped += self.set_since_paint - 1;
        self.set_since_paint = 0;
        if let Some(what) = self.awaiting.take() {
            perf!("frame-first-presented", "{what}");
        }
        let since = *self.since.get_or_insert_with(Instant::now);
        let secs = since.elapsed().as_secs_f64();
        if secs >= 2.0 {
            perf!(
                "viewfinder",
                "delivered_fps={:.1} presented_fps={:.1} dropped={} main_ms_avg={:.2} main_ms_max={:.2} rss_mib={:.0}",
                self.delivered as f64 / secs,
                self.presented as f64 / secs,
                self.dropped,
                self.main_ns as f64 / self.delivered.max(1) as f64 / 1e6,
                self.main_max_ns as f64 / 1e6,
                crate::perf::rss_mib()
            );
            *self = PerfStats { awaiting: self.awaiting.take(), shutter: self.shutter, since: Some(Instant::now()), ..Default::default() };
        }
    }
}

#[derive(Debug)]
pub enum Msg {
    Camera(Event),
    SwitchCamera,
    SelectMode(Mode),
    SelectModeIndex(u32),
    /// The shutter: starts the self-timer when one is set.
    Capture,
    Countdown,
    OpenLast,
    Retry,
    OpenSettings,
    ToggleControls,
    ShowControl(&'static [&'static str]),
    Saved(Result<(PathBuf, Thumb), String>),
    /// The newest photo already on disk, found at startup.
    Found(PathBuf, Thumb),
    SetVideo(bool),
    RecordingStarted(Result<Arc<Recorder>, String>),
    RecordingDone(Result<PathBuf, String>),
    Tick,
    CycleTimer,
    PortalSlow,
    TapFocus(f64, f64),
    HideFocus(u32),
    /// Hold focus and exposure where the viewfinder was long-pressed.
    Lock(f64, f64),
    LockExposure(f64),
    Suspended(bool),
    /// Degrees the device is turned clockwise from its natural orientation.
    Orientation(i32),
    ZoomBegin,
    Pinch(f64),
    CycleZoom,
    Preferences,
}

#[derive(Debug)]
pub enum CmdOut {
    Access(Access),
}

/// Tight RGBA rows for the gallery button.
pub struct Thumb {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    rotation: i32,
}

impl std::fmt::Debug for Thumb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Thumb({}x{})", self.width, self.height)
    }
}

/// Current values over the viewfinder, each opening its control.
pub struct Chips {
    iso: gtk::Button,
    shutter: gtk::Button,
    ev: gtk::Button,
    wb: gtk::Button,
    focus: gtk::Button,
    fps: gtk::Button,
    zoom: gtk::Button,
}

pub struct Widgets {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    status: adw::StatusPage,
    retry: gtk::Button,
    open_settings: gtk::Button,
    viewfinder: Viewfinder,
    grid: gtk::DrawingArea,
    focus_layer: gtk::Fixed,
    focus_ring: gtk::Box,
    countdown: gtk::Label,
    controls_title: adw::WindowTitle,
    controls_toggle: gtk::ToggleButton,
    controls_box: gtk::Box,
    capture_group: adw::PreferencesGroup,
    mode_row: adw::ComboRow,
    mode_handler: glib::SignalHandlerId,
    raw_row: adw::SwitchRow,
    raw_toggle: gtk::ToggleButton,
    quick: gtk::Box,
    resolution: gtk::MenuButton,
    resolution_action: gio::SimpleAction,
    timer_button: gtk::Button,
    timer_label: gtk::Label,
    switch: gtk::Button,
    capture: gtk::Button,
    modes: adw::ToggleGroup,
    record_pill: gtk::Box,
    lock_pill: gtk::Box,
    lock_label: gtk::Label,
    lock_ev: gtk::Scale,
    record_time: gtk::Label,
    thumbnail: Viewfinder,
    gallery: gtk::Button,
    saving_spinner: adw::Spinner,
    gallery_placeholder: gtk::Image,
    chips: Chips,
}

fn aspect(m: &Mode) -> String {
    let r = m.width as f64 / m.height.max(1) as f64;
    match () {
        _ if (r - 4.0 / 3.0).abs() < 0.02 => "4:3".into(),
        _ if (r - 16.0 / 9.0).abs() < 0.03 => "16:9".into(),
        _ if (r - 3.0 / 2.0).abs() < 0.02 => "3:2".into(),
        _ if (r - 1.0).abs() < 0.02 => "1:1".into(),
        _ => format!("{r:.2}:1"),
    }
}

fn megapixels(m: &Mode) -> String {
    let mp = m.width as f64 * m.height as f64 / 1e6;
    if mp >= 9.5 { format!("{mp:.0} MP") } else { format!("{mp:.1} MP") }
}

fn mode_label(m: &Mode) -> String {
    format!("{} · {} · {} × {}", aspect(m), megapixels(m), m.width, m.height)
}

/// "4K", "1080p": what a recording at this sensor mode is called.
fn video_name(m: &Mode) -> String {
    match m.width {
        3840.. => "4K".into(),
        1920.. => "1080p".into(),
        1280.. => "720p".into(),
        _ => format!("{}p", m.height),
    }
}

fn wide(m: &Mode) -> bool {
    (m.width as f64 / m.height.max(1) as f64 - 16.0 / 9.0).abs() < 0.05
}

/// Say an icon-only widget's name to assistive technologies.
fn label(widget: &impl IsA<gtk::Accessible>, text: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(text)]);
}

fn icon_button(icon: &str, tooltip: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).css_classes(classes.to_vec()).build();
    label(&b, tooltip);
    b
}

/// A value over the viewfinder. `chars` fits its widest value, so a changing
/// number never moves its neighbours.
fn chip(tooltip: &str, sender: &ComponentSender<App>, rows: &'static [&'static str], chars: i32) -> gtk::Button {
    let text = gtk::Label::builder().width_chars(chars).max_width_chars(chars).css_classes(["numeric"]).build();
    let b = gtk::Button::builder().child(&text).tooltip_text(tooltip).css_classes(["chip"]).visible(false).build();
    label(&b, tooltip);
    let s = sender.clone();
    b.connect_clicked(move |_| s.input(Msg::ShowControl(rows)));
    b
}

fn set_chip(b: &gtk::Button, text: &str, manual: bool) {
    if let Some(l) = b.child().and_downcast::<gtk::Label>()
        && l.label() != text
    {
        l.set_label(text);
    }
    if manual { b.add_css_class("manual") } else { b.remove_css_class("manual") }
}

/// A boolean setting as a stateful action; plain state when the schema is
/// not installed (running from the build tree).
fn toggle_action(settings: Option<&gio::Settings>, key: &str) -> gio::Action {
    match settings {
        Some(s) => s.create_action(key),
        None => {
            let a = gio::SimpleAction::new_stateful(key, None, &false.to_variant());
            a.connect_activate(|a, _| {
                let on = a.state().and_then(|v| v.get::<bool>()).unwrap_or(false);
                a.set_state(&(!on).to_variant());
            });
            a.upcast()
        }
    }
}

fn action_bool(a: &gio::Action) -> bool {
    a.state().and_then(|v| v.get::<bool>()).unwrap_or(false)
}

impl App {
    fn backend(&self) -> Option<&LibcameraBackend> {
        self.backend.as_deref()
    }

    fn saved_mode(&self, id: &str) -> Option<Mode> {
        let s = self.settings.as_ref()?;
        let modes: std::collections::HashMap<String, String> = s.get("modes");
        let (w, h) = modes.get(id)?.split_once('x')?;
        Some(Mode { width: w.parse().ok()?, height: h.parse().ok()? })
    }

    fn remember(&self, session: &Session) {
        let Some(s) = self.settings.as_ref() else { return };
        let mut modes: std::collections::HashMap<String, String> = s.get("modes");
        modes.insert(session.info.id.clone(), format!("{}x{}", session.mode.width, session.mode.height));
        let _ = s.set("modes", &modes);
        let _ = s.set_string("camera", &session.info.id);
    }

    /// Close the session and open `index` in `mode`; the viewfinder goes
    /// dark until the first frame fades it back in.
    fn reopen(&mut self, w: &Widgets, index: usize, mode: Option<Mode>) {
        w.capture.set_sensitive(false);
        w.viewfinder.add_css_class("switching");
        w.viewfinder.set_texture(None);
        w.focus_ring.set_visible(false);
        // The new session's controls start automatic.
        self.locked = false;
        self.lock_exposure = None;
        w.lock_pill.set_visible(false);
        perf!("camera-open-request", "camera={index}");
        if let Some(p) = &self.perf {
            p.borrow_mut().awaiting = Some(format!("camera={index}"));
        }
        // A still requested from the closing session may never arrive.
        self.saving = false;
        w.saving_spinner.set_visible(false);
        self.session = None;
        self.awaiting_frame = true;
        self.camera = index;
        let mode = mode.or_else(|| self.cameras.get(index).and_then(|c| self.saved_mode(&c.id)));
        if let Some(b) = self.backend() {
            b.send(Cmd::Open { camera: index, mode, video: self.video });
        }
    }

    /// `icon` None shows a spinner.
    fn show_status(&self, w: &Widgets, icon: Option<&str>, title: &str, description: Option<&str>, retry: bool) {
        match icon {
            Some(icon) => w.status.set_icon_name(Some(icon)),
            None => w.status.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&w.status)))),
        }
        w.status.set_title(title);
        w.status.set_description(description);
        w.retry.set_visible(retry);
        w.open_settings.set_visible(false);
        w.quick.set_visible(false);
        w.stack.set_visible_child_name("status");
    }

    fn raw_enabled(&self) -> bool {
        relm4::main_application().lookup_action("raw").is_some_and(|a| action_bool(&a))
    }

    fn update_chips(&self, c: &Chips, meta: &Metadata) {
        let Some(panel) = &self.panel else { return };
        let manual = panel.manual();
        if let Some(g) = meta.get("AnalogueGain") {
            set_chip(&c.iso, &format!("ISO {:.0}", g * meta.get("DigitalGain").unwrap_or(1.0) * 100.0), manual.gain);
        }
        if let Some(us) = meta.get("ExposureTime") {
            let t = format_value("ExposureTime", us);
            let t = match t.strip_suffix(" s") {
                Some(v) if v.contains('/') => v.to_string(),
                Some(v) => format!("{v}″"),
                None => t,
            };
            set_chip(&c.shutter, &t, manual.exposure);
        }
        if let Some(ev) = panel.value("ExposureValue") {
            let text = if ev == 0.0 { "±0".to_string() } else { format!("{ev:+.1}") };
            set_chip(&c.ev, &text, ev != 0.0);
        }
        match meta.get("ColourTemperature") {
            Some(k) => set_chip(&c.wb, &format!("{k:.0}K"), manual.white_balance),
            None => set_chip(&c.wb, &gettext("AWB"), manual.white_balance),
        }
        match (manual.focus, meta.get("LensPosition")) {
            (true, _) if self.locked => set_chip(&c.focus, &gettext("AF-L"), true),
            (true, Some(d)) => set_chip(&c.focus, &format!("MF {}", format_value("LensPosition", d).replace(' ', "")), true),
            // Single-shot after a tap, continuous otherwise.
            _ if panel.is("AfMode", "Auto") => set_chip(&c.focus, &gettext("AF-S"), true),
            _ => set_chip(&c.focus, &gettext("AF"), false),
        }
        set_chip(&c.fps, &format!("{:.0} fps", self.fps), false);
    }

    /// Press the shutter for real: a photo, or start/stop a recording.
    fn shoot(&mut self, w: &Widgets, sender: &ComponentSender<Self>) {
        let Some(session) = &self.session else { return };
        if self.video {
            if let Some(recorder) = self.recorder.take() {
                perf!("record-stop-press");
                self.feedback();
                w.capture.set_sensitive(false);
                if let Some(b) = self.backend() {
                    b.send(Cmd::Record(None));
                }
                let input = sender.input_sender().clone();
                std::thread::spawn(move || {
                    let _ = input.send(Msg::RecordingDone(recorder.stop().map_err(|e| e.to_string())));
                });
            } else if self.record_started.is_none() {
                perf!("record-press");
                w.capture.set_sensitive(false);
                self.record_started = Some(Instant::now());
                let rotation = crate::device::upright(session.info.rotation, session.info.facing == Facing::Front, self.device);
                let (view, fourcc) = (session.view, session.fourcc);
                self.feedback();
                let fps = self.frame_duration.map(|us| 1e6 / us).or(session.fps.map(|f| f.1)).unwrap_or(30.0);
                let input = sender.input_sender().clone();
                std::thread::spawn(move || {
                    let r = Recorder::start(view.width, view.height, fourcc, fps, rotation).map(Arc::new).map_err(|e| e.to_string());
                    let _ = input.send(Msg::RecordingStarted(r));
                });
            }
        } else if !self.saving {
            perf!("shutter");
            // The thumbnail is what was on screen, right away; the saved
            // photo's replaces it when written.
            let front = session.info.facing == Facing::Front;
            if let Some(copy) = w.viewfinder.freeze() {
                w.thumbnail.set_texture(Some(copy));
                w.thumbnail.set_rotation(0, front);
                w.gallery_placeholder.set_visible(false);
                w.gallery.set_sensitive(true);
                w.gallery.add_css_class("new");
                let g = w.gallery.clone();
                glib::timeout_add_local_once(Duration::from_millis(400), move || g.remove_css_class("new"));
                perf!("thumbnail-preview");
                if w.controls_toggle.is_active() {
                    // The gallery button is under the controls.
                    let toast = adw::Toast::builder().title(gettext("Photo taken")).button_label(gettext("_Open")).action_name("app.open-last").timeout(3).build();
                    w.toasts.add_toast(toast);
                }
            }
            if let Some(p) = &self.perf {
                p.borrow_mut().shutter = Some(Instant::now());
            }
            self.saving = true;
            w.capture.set_sensitive(false);
            w.saving_spinner.set_visible(true);
            w.viewfinder.add_css_class("flash");
            let vf = w.viewfinder.clone();
            glib::timeout_add_local_once(Duration::from_millis(90), move || vf.remove_css_class("flash"));
            if let Some(b) = self.backend() {
                b.send(Cmd::Capture);
                self.feedback();
            }
        }
    }

    fn set_recording_ui(&self, w: &Widgets, on: bool) {
        for widget in [w.modes.upcast_ref::<gtk::Widget>(), w.switch.upcast_ref(), w.mode_row.upcast_ref(), w.resolution.upcast_ref(), w.timer_button.upcast_ref()] {
            widget.set_sensitive(!on);
        }
        w.record_pill.set_visible(on);
        w.capture.set_icon_name(if on { "media-playback-stop-symbolic" } else { "media-record-symbolic" });
        let tip = if on { gettext("Stop Recording") } else { gettext("Start Recording") };
        w.capture.set_tooltip_text(Some(&tip));
        label(&w.capture, &tip);
        if on { w.capture.add_css_class("recording") } else { w.capture.remove_css_class("recording") }
    }

    fn show_timer(&self, w: &Widgets) {
        w.timer_label.set_label(&format!("{} s", self.timer));
        w.timer_label.set_visible(self.timer > 0);
        let tip = match self.timer {
            0 => gettext("Self-Timer: Off"),
            n => gettext("Self-Timer: {} Seconds").replace("{}", &n.to_string()),
        };
        w.timer_button.set_tooltip_text(Some(&tip));
        label(&w.timer_button, &tip);
        if self.timer > 0 { w.timer_button.add_css_class("on") } else { w.timer_button.remove_css_class("on") }
    }
}

/// The newest photo already in the gallery, as a thumbnail.
fn last_photo() -> Option<(PathBuf, Thumb)> {
    let path = std::fs::read_dir(crate::photo::pictures_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg")))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?
        .path();
    let pixbuf = gdk::gdk_pixbuf::Pixbuf::from_file_at_scale(&path, 128, 128, true).ok()?.add_alpha(false, 0, 0, 0).ok()?;
    let (w, h, stride) = (pixbuf.width() as usize, pixbuf.height() as usize, pixbuf.rowstride() as usize);
    let bytes = pixbuf.read_pixel_bytes();
    let rgba = (0..h).flat_map(|y| bytes[y * stride..y * stride + w * 4].to_vec()).collect();
    Some((path, Thumb { rgba, width: w as u32, height: h as u32, rotation: 0 }))
}

impl Component for App {
    type Init = ();
    type Input = Msg;
    type Output = ();
    type CommandOutput = CmdOut;
    type Widgets = Widgets;
    type Root = adw::ApplicationWindow;

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .title(gettext("Obscura"))
            .default_width(960)
            .default_height(680)
            .width_request(360)
            .height_request(294)
            .build()
    }

    fn init(_: (), window: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        perf!("app-init");
        // Cameras are enumerated while the portal asks; none is opened
        // until it answers.
        let backend = {
            let input = sender.input_sender().clone();
            Rc::new(LibcameraBackend::spawn(move |ev| {
                let _ = input.send(Msg::Camera(ev));
            }))
        };
        // Pictures read best against dark surroundings.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        let settings = gio::SettingsSchemaSource::default()
            .and_then(|src| src.lookup(APP_ID, true))
            .map(|_| gio::Settings::new(APP_ID));
        let app = relm4::main_application();
        if let Some(s) = &settings {
            s.bind("window-width", &window, "default-width").build();
            s.bind("window-height", &window, "default-height").build();
            s.bind("window-maximized", &window, "maximized").build();
        }

        let grid_action = toggle_action(settings.as_ref(), "grid");
        let raw_action = toggle_action(settings.as_ref(), "raw");
        let info_action = toggle_action(settings.as_ref(), "show-info");
        for a in [&grid_action, &raw_action, &info_action] {
            app.add_action(a);
        }

        // Header: quick settings over the viewfinder
        let grid_toggle = gtk::ToggleButton::builder().icon_name("view-grid-symbolic").tooltip_text(gettext("Grid")).action_name("app.grid").build();
        label(&grid_toggle, &gettext("Grid"));
        let timer_label = gtk::Label::builder().css_classes(["numeric"]).visible(false).build();
        let timer_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        timer_content.append(&gtk::Image::from_icon_name("alarm-symbolic"));
        timer_content.append(&timer_label);
        let timer_button = gtk::Button::builder().child(&timer_content).css_classes(["flat", "timer"]).build();
        let raw_toggle = gtk::ToggleButton::builder()
            .label(gettext("RAW"))
            .tooltip_text(gettext("Also Save RAW (DNG)"))
            .action_name("app.raw")
            .css_classes(["raw-toggle"])
            .visible(false)
            .build();
        let quick = gtk::Box::builder().spacing(2).visible(false).build();
        quick.append(&grid_toggle);
        quick.append(&timer_button);
        quick.append(&raw_toggle);
        let resolution = gtk::MenuButton::builder().label("4:3").tooltip_text(gettext("Resolution")).css_classes(["flat", "numeric"]).build();
        quick.append(&resolution);

        let controls_toggle = gtk::ToggleButton::builder().icon_name("preferences-system-symbolic").tooltip_text(gettext("Camera Controls")).build();
        label(&controls_toggle, &gettext("Camera Controls"));
        quick.bind_property("visible", &controls_toggle, "visible").sync_create().build();
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some(&gettext("Show Capture _Info")), Some("app.show-info"));
        menu.append_section(None, &section);
        let section = gio::Menu::new();
        section.append(Some(&gettext("_Preferences")), Some("app.preferences"));
        section.append(Some(&gettext("_Keyboard Shortcuts")), Some("app.shortcuts"));
        section.append(Some(&gettext("_About Obscura")), Some("app.about"));
        menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).tooltip_text(gettext("Main Menu")).build();
        label(&menu_button, &gettext("Main Menu"));
        // No title: a phone has room for the quick settings, not a name.
        let header = adw::HeaderBar::builder().title_widget(&gtk::Box::new(gtk::Orientation::Horizontal, 0)).build();
        header.pack_start(&quick);
        header.pack_end(&menu_button);
        header.pack_end(&controls_toggle);

        // Viewfinder page
        let viewfinder = Viewfinder::default();
        viewfinder.set_hexpand(true);
        viewfinder.set_vexpand(true);
        viewfinder.add_css_class("viewfinder");
        label(&viewfinder, &gettext("Viewfinder"));
        {
            let s = sender.clone();
            let tap = gtk::GestureClick::new();
            tap.connect_released(move |g, n, x, y| {
                if n == 1 {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    s.input(Msg::TapFocus(x, y));
                }
            });
            viewfinder.add_controller(tap);
            // Swipe sideways between photo and video, like phone cameras;
            // hold to go back to continuous focus after a tap.
            let swipe = gtk::GestureSwipe::new();
            swipe.connect_swipe(|_, vx, vy| {
                if vx.abs() > 500.0 && vx.abs() > 2.0 * vy.abs() {
                    let mode = if vx < 0.0 { "video" } else { "photo" };
                    relm4::main_application().activate_action("mode", Some(&mode.to_variant()));
                }
            });
            viewfinder.add_controller(swipe);
            let pinch = gtk::GestureZoom::new();
            let s = sender.clone();
            pinch.connect_begin(move |_, _| s.input(Msg::ZoomBegin));
            let s = sender.clone();
            pinch.connect_scale_changed(move |_, scale| s.input(Msg::Pinch(scale)));
            viewfinder.add_controller(pinch);
            let s = sender.clone();
            let hold = gtk::GestureLongPress::new();
            hold.connect_pressed(move |_, x, y| s.input(Msg::Lock(x, y)));
            viewfinder.add_controller(hold);
        }

        let grid = viewfinder::grid(&viewfinder);
        grid_action
            .bind_property("state", &grid, "visible")
            .transform_to(|_, v: glib::Variant| v.get::<bool>())
            .sync_create()
            .build();
        let focus_ring = gtk::Box::builder().css_classes(["focus-ring"]).width_request(76).height_request(76).visible(false).build();
        let focus_layer = gtk::Fixed::builder().can_target(false).build();
        focus_layer.put(&focus_ring, 0.0, 0.0);
        let countdown = gtk::Label::builder().css_classes(["countdown", "numeric"]).halign(gtk::Align::Center).valign(gtk::Align::Center).can_target(false).visible(false).build();

        let record_time = gtk::Label::builder().label("0:00").css_classes(["numeric"]).build();
        let record_pill = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .css_classes(["recording-pill"])
            .visible(false)
            .build();
        record_pill.append(&gtk::Box::builder().css_classes(["rec-dot"]).valign(gtk::Align::Center).build());
        record_pill.append(&record_time);

        // AE/AF lock: what is held, and exposure compensation for it.
        let lock_label = gtk::Label::new(None);
        let lock_ev = gtk::Scale::with_range(gtk::Orientation::Horizontal, -2.0, 2.0, 0.1);
        lock_ev.set_value(0.0);
        lock_ev.add_mark(0.0, gtk::PositionType::Bottom, None);
        lock_ev.set_draw_value(false);
        lock_ev.set_width_request(150);
        label(&lock_ev, &gettext("Exposure Compensation"));
        {
            let s = sender.clone();
            lock_ev.connect_value_changed(move |sc| s.input(Msg::LockExposure(sc.value())));
        }
        let lock_pill = gtk::Box::builder()
            .spacing(10)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(52)
            .css_classes(["lock-pill"])
            .visible(false)
            .build();
        lock_pill.append(&lock_label);
        lock_pill.append(&lock_ev);
        let chips = Chips {
            zoom: {
                let text = gtk::Label::builder().label("1×").width_chars(4).max_width_chars(4).css_classes(["numeric"]).build();
                let b = gtk::Button::builder().child(&text).tooltip_text(gettext("Zoom")).css_classes(["chip", "zoom"]).build();
                label(&b, &gettext("Zoom"));
                let s = sender.clone();
                b.connect_clicked(move |_| s.input(Msg::CycleZoom));
                b
            },
            iso: chip(&gettext("ISO"), &sender, &["AnalogueGainMode", "AnalogueGain", "AeEnable"], 8),
            shutter: chip(&gettext("Shutter Speed"), &sender, &["ExposureTimeMode", "ExposureTime", "AeEnable"], 6),
            ev: chip(&gettext("Exposure Compensation"), &sender, &["ExposureValue"], 4),
            wb: chip(&gettext("White Balance"), &sender, &["AwbMode", "AwbEnable", "ColourTemperature"], 5),
            focus: chip(&gettext("Focus"), &sender, &["AfMode", "LensPosition"], 7),
            fps: chip(&gettext("Frame Rate"), &sender, &["FrameDurationLimits"], 6),
        };
        let strip = gtk::Box::builder().spacing(4).halign(gtk::Align::Center).build();
        for c in [&chips.zoom, &chips.iso, &chips.shutter, &chips.ev, &chips.wb, &chips.focus, &chips.fps] {
            strip.append(c);
        }
        let chip_scroller = gtk::ScrolledWindow::builder()
            .child(&strip)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .build();
        info_action
            .bind_property("state", &chip_scroller, "visible")
            .transform_to(|_, v: glib::Variant| v.get::<bool>())
            .sync_create()
            .build();

        let capture = gtk::Button::builder()
            .icon_name("camera-photo-symbolic")
            .tooltip_text(gettext("Take Photo"))
            .css_classes(["shutter"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        label(&capture, &gettext("Take Photo"));
        let switch = icon_button("camera-switch-symbolic", &gettext("Switch Camera"), &["round-button", "switch-camera"]);
        switch.set_visible(false);
        switch.set_halign(gtk::Align::Center);
        let thumbnail = Viewfinder::default();
        thumbnail.set_size_request(52, 52);
        thumbnail.set_cover(true);
        thumbnail.set_overflow(gtk::Overflow::Hidden);
        let saving_spinner = adw::Spinner::builder().visible(false).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        let gallery_content = gtk::Overlay::builder().child(&thumbnail).build();
        let gallery_placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
        gallery_content.add_overlay(&gallery_placeholder);
        gallery_content.add_overlay(&saving_spinner);
        let gallery = gtk::Button::builder()
            .child(&gallery_content)
            .tooltip_text(gettext("Open Last Capture"))
            .css_classes(["round-button", "gallery"])
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .sensitive(false)
            .build();
        label(&gallery, &gettext("Open Last Capture"));
        let buttons = gtk::CenterBox::builder().start_widget(&gallery).center_widget(&capture).end_widget(&switch).build();
        let modes = adw::ToggleGroup::builder().halign(gtk::Align::Center).css_classes(["round", "mode-switch"]).build();
        modes.add(adw::Toggle::builder().name("photo").label(gettext("Photo")).build());
        modes.add(adw::Toggle::builder().name("video").label(gettext("Video")).build());
        modes.set_active_name(Some("photo"));
        let bar_content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(14).build();
        bar_content.append(&chip_scroller);
        bar_content.append(&modes);
        bar_content.append(&buttons);
        // Full-width shade, thumb-width controls.
        let bar = adw::Clamp::builder().maximum_size(460).tightening_threshold(460).child(&bar_content).valign(gtk::Align::End).css_classes(["capture-bar"]).build();
        let side_bar = |bp: &adw::Breakpoint| {
            // Landscape phones: the controls stand in a column on the right.
            bp.add_setter(&bar, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            bp.add_setter(&bar, "valign", Some(&gtk::Align::Fill.to_value()));
            bp.add_setter(&bar, "halign", Some(&gtk::Align::End.to_value()));
            bp.add_setter(&bar, "css-classes", Some(&vec!["capture-bar-side".to_string()].to_value()));
            bp.add_setter(&bar_content, "orientation", Some(&gtk::Orientation::Horizontal.to_value()));
            bp.add_setter(&strip, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            bp.add_setter(&chip_scroller, "hscrollbar-policy", Some(&gtk::PolicyType::Never.to_value()));
            bp.add_setter(&chip_scroller, "vscrollbar-policy", Some(&gtk::PolicyType::External.to_value()));
            bp.add_setter(&chip_scroller, "valign", Some(&gtk::Align::Center.to_value()));
            bp.add_setter(&chip_scroller, "propagate-natural-height", Some(&true.to_value()));
            bp.add_setter(&chip_scroller, "propagate-natural-width", Some(&true.to_value()));
            bp.add_setter(&modes, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
            bp.add_setter(&modes, "valign", Some(&gtk::Align::Center.to_value()));
            bp.add_setter(&buttons, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
        };

        // Offloaded, the viewfinder's dmabufs can go to the compositor as a
        // subsurface (even a display plane) instead of through the GPU.
        let offload = gtk::GraphicsOffload::builder().child(&viewfinder).black_background(true).build();
        let overlay = gtk::Overlay::builder().child(&offload).build();
        overlay.add_overlay(&grid);
        overlay.add_overlay(&focus_layer);
        overlay.add_overlay(&countdown);
        overlay.add_overlay(&record_pill);
        overlay.add_overlay(&lock_pill);
        overlay.add_overlay(&bar);

        // Status page: loading, permission, no camera, errors
        let retry = gtk::Button::builder().label(gettext("_Try Again")).use_underline(true).css_classes(["pill"]).visible(false).build();
        let open_settings = gtk::Button::builder().label(gettext("Open _Settings")).use_underline(true).css_classes(["pill", "suggested-action"]).visible(false).build();
        let status_buttons = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).halign(gtk::Align::Center).build();
        status_buttons.append(&open_settings);
        status_buttons.append(&retry);
        let status = adw::StatusPage::builder().title(gettext("Starting Camera…")).child(&status_buttons).build();
        status.set_paintable(Some(&adw::SpinnerPaintable::new(Some(&status))));

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&overlay, Some("camera"));
        let camera_page = adw::ToolbarView::builder().content(&stack).css_classes(["camera"]).build();
        camera_page.add_top_bar(&header);

        // Controls: a sidebar when wide, a bottom sheet when narrow
        let mode_row = adw::ComboRow::builder().title(gettext("Resolution")).build();
        let raw_row = adw::SwitchRow::builder().title(gettext("Save RAW")).subtitle(gettext("Also write a DNG next to each photo")).visible(false).build();
        if let Some(s) = &settings {
            s.bind("raw", &raw_row, "active").build();
        }
        let capture_group = adw::PreferencesGroup::builder().title(gettext("Capture")).build();
        capture_group.add(&mode_row);
        capture_group.add(&raw_row);
        let controls_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let controls_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(6)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        controls_content.append(&capture_group);
        controls_content.append(&controls_box);
        let controls_scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .vexpand(true)
            .child(&adw::Clamp::builder().maximum_size(480).child(&controls_content).build())
            .build();
        let controls_title = adw::WindowTitle::new(&gettext("Camera Controls"), "");
        let controls_header = adw::HeaderBar::builder().title_widget(&controls_title).build();
        // Shoot without closing the controls.
        let sheet_capture = icon_button("camera-photo-symbolic", &gettext("Take Photo"), &[]);
        sheet_capture.set_action_name(Some("app.capture"));
        controls_header.pack_start(&sheet_capture);
        let controls_page = adw::ToolbarView::builder().content(&controls_scroller).build();
        controls_page.add_top_bar(&controls_header);

        let split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .sidebar(&adw::LayoutSlot::new("controls"))
            .content(&adw::LayoutSlot::new("camera"))
            .show_sidebar(false)
            .min_sidebar_width(320.0)
            .max_sidebar_width(400.0)
            .build();
        let sheet = adw::BottomSheet::builder()
            .content(&adw::LayoutSlot::new("camera"))
            .sheet(&adw::LayoutSlot::new("controls"))
            .modal(false)
            .show_drag_handle(true)
            .build();
        split.bind_property("show-sidebar", &controls_toggle, "active").bidirectional().sync_create().build();
        sheet.bind_property("open", &controls_toggle, "active").bidirectional().build();
        let layouts = adw::MultiLayoutView::new();
        let wide_layout = adw::Layout::new(&split);
        wide_layout.set_name(Some("wide"));
        let narrow_layout = adw::Layout::new(&sheet);
        narrow_layout.set_name(Some("narrow"));
        layouts.add_layout(wide_layout);
        layouts.add_layout(narrow_layout);
        layouts.set_child("camera", &camera_page);
        layouts.set_child("controls", &controls_page);
        layouts.set_layout_name("wide");

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&layouts));
        window.set_content(Some(&toasts));

        let narrow = || adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxWidth, 720.0, adw::LengthUnit::Sp);
        let landscape = || {
            adw::BreakpointCondition::new_and(
                adw::BreakpointCondition::new_length(adw::BreakpointConditionLengthType::MaxHeight, 520.0, adw::LengthUnit::Sp),
                adw::BreakpointCondition::new_ratio(adw::BreakpointConditionRatioType::MinAspectRatio, 4, 3),
            )
        };
        let sheet_setters = |bp: &adw::Breakpoint| {
            bp.add_setter(&layouts, "layout-name", Some(&"narrow".to_value()));
            // In the sheet the controls take part of the screen, so the
            // picture being adjusted stays in sight.
            bp.add_setter(&controls_scroller, "max-content-height", Some(&340.to_value()));
        };
        // When several match, the last one added applies.
        let bp = adw::Breakpoint::new(narrow());
        sheet_setters(&bp);
        window.add_breakpoint(bp);
        let bp = adw::Breakpoint::new(landscape());
        side_bar(&bp);
        window.add_breakpoint(bp);
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_and(narrow(), landscape()));
        sheet_setters(&bp);
        side_bar(&bp);
        window.add_breakpoint(bp);

        // Actions
        let about = gio::SimpleAction::new("about", None);
        {
            let window = window.clone();
            about.connect_activate(move |_, _| {
                adw::AboutDialog::builder()
                    .application_name(gettext("Obscura"))
                    .application_icon(APP_ID)
                    .developer_name("Giuseppe Maggio")
                    .version(env!("CARGO_PKG_VERSION"))
                    .license_type(gtk::License::Gpl30)
                    .comments(gettext("Take pictures and videos, with every control your camera has"))
                    .build()
                    .present(Some(&window));
            });
        }
        app.add_action(&about);
        let shortcuts = gio::SimpleAction::new("shortcuts", None);
        {
            let window = window.clone();
            shortcuts.connect_activate(move |_, _| {
                let dialog = adw::ShortcutsDialog::new();
                let section = adw::ShortcutsSection::new(None);
                for (title, action) in [
                    (gettext("Take Photo or Record"), "app.capture"),
                    (gettext("Photo Mode"), "app.mode::photo"),
                    (gettext("Video Mode"), "app.mode::video"),
                    (gettext("Switch Camera"), "app.switch-camera"),
                    (gettext("Camera Controls"), "app.toggle-controls"),
                    (gettext("Grid"), "app.grid"),
                    (gettext("Self-Timer"), "app.timer"),
                    (gettext("Show Capture Info"), "app.show-info"),
                    (gettext("Open Last Capture"), "app.open-last"),
                    (gettext("Zoom"), "app.zoom"),
                    (gettext("Preferences"), "app.preferences"),
                    (gettext("Keyboard Shortcuts"), "app.shortcuts"),
                    (gettext("Quit"), "app.quit"),
                ] {
                    section.add(adw::ShortcutsItem::from_action(&title, action));
                }
                dialog.add(section);
                dialog.present(Some(&window));
            });
        }
        app.add_action(&shortcuts);
        let mode_action = gio::SimpleAction::new_stateful("mode", Some(glib::VariantTy::STRING), &"photo".to_variant());
        {
            let modes = modes.clone();
            mode_action.connect_activate(move |a, v| {
                if let Some(name) = v.and_then(|v| v.get::<String>()) {
                    a.set_state(&name.to_variant());
                    modes.set_active_name(Some(&name));
                }
            });
        }
        app.add_action(&mode_action);
        let resolution_action = gio::SimpleAction::new_stateful("resolution", Some(glib::VariantTy::STRING), &"".to_variant());
        {
            let s = sender.clone();
            resolution_action.connect_activate(move |_, v| {
                let parsed = v.and_then(|v| v.get::<String>()).and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some(Mode { width: w.parse().ok()?, height: h.parse().ok()? })
                });
                if let Some(mode) = parsed {
                    s.input(Msg::SelectMode(mode));
                }
            });
        }
        app.add_action(&resolution_action);

        // Everything the capture bar does is also an action: keyboard
        // accelerators, and scriptable over D-Bus (org.gtk.Actions).
        for (name, accels, msg) in [
            ("capture", &["space", "Return"][..], (|| Msg::Capture) as fn() -> Msg),
            ("toggle-controls", &["F9"][..], || Msg::ToggleControls),
            ("switch-camera", &["<Ctrl>Tab"][..], || Msg::SwitchCamera),
            ("open-last", &["<Ctrl>o"][..], || Msg::OpenLast),
            ("timer", &["<Ctrl>t"][..], || Msg::CycleTimer),
            ("zoom", &["<Ctrl>plus"][..], || Msg::CycleZoom),
            ("preferences", &["<Ctrl>comma"][..], || Msg::Preferences),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let s = sender.clone();
            action.connect_activate(move |_, _| s.input(msg()));
            app.add_action(&action);
            app.set_accels_for_action(&format!("app.{name}"), accels);
        }
        let quit = gio::SimpleAction::new("quit", None);
        {
            let window = window.clone();
            quit.connect_activate(move |_, _| window.close());
        }
        app.add_action(&quit);
        for (action, accels) in [
            ("app.quit", &["<Ctrl>q"][..]),
            ("app.shortcuts", &["<Ctrl>question"][..]),
            ("app.grid", &["<Ctrl>g"][..]),
            ("app.show-info", &["<Ctrl>i"][..]),
            ("app.mode::photo", &["<Ctrl>1"][..]),
            ("app.mode::video", &["<Ctrl>2"][..]),
        ] {
            app.set_accels_for_action(action, accels);
        }

        // Signals
        {
            let s = sender.clone();
            capture.connect_clicked(move |_| s.input(Msg::Capture));
            let s = sender.clone();
            switch.connect_clicked(move |_| s.input(Msg::SwitchCamera));
            let s = sender.clone();
            gallery.connect_clicked(move |_| s.input(Msg::OpenLast));
            let s = sender.clone();
            retry.connect_clicked(move |_| s.input(Msg::Retry));
            let s = sender.clone();
            open_settings.connect_clicked(move |_| s.input(Msg::OpenSettings));
            let s = sender.clone();
            timer_button.connect_clicked(move |_| s.input(Msg::CycleTimer));
            let s = sender.clone();
            modes.connect_active_name_notify(move |g| s.input(Msg::SetVideo(g.active_name().as_deref() == Some("video"))));
        }
        {
            let s = sender.clone();
            window.connect_suspended_notify(move |w| s.input(Msg::Suspended(w.is_suspended())));
        }
        let mode_handler = {
            let s = sender.clone();
            mode_row.connect_selected_notify(move |r| s.input(Msg::SelectModeIndex(r.selected())))
        };
        {
            let s = sender.clone();
            glib::timeout_add_local_once(Duration::from_millis(600), move || s.input(Msg::PortalSlow));
        }

        sender.oneshot_command(async { CmdOut::Access(portal::request_access().await) });
        {
            let input = sender.input_sender().clone();
            std::thread::spawn(move || {
                if let Some((path, thumb)) = last_photo() {
                    let _ = input.send(Msg::Found(path, thumb));
                }
            });
        }

        let video = settings.as_ref().is_some_and(|s| s.boolean("video"));
        if let Some(s) = &settings {
            backend.send(Cmd::FullResolution(s.boolean("full-resolution")));
            let b = backend.clone();
            s.connect_changed(Some("full-resolution"), move |s, k| b.send(Cmd::FullResolution(s.boolean(k))));
        }
        let timer = settings.as_ref().map(|s| s.int("timer").max(0) as u32).unwrap_or(0);
        let model = App {
            backend: Some(backend),
            cameras: Vec::new(),
            camera: 0,
            session: None,
            panel: None,
            copy_frames: false,
            awaiting_frame: true,
            frames: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            last_capture: None,
            settings,
            saving: false,
            video: false,
            recorder: None,
            record_started: None,
            record_tick: None,
            frame_duration: None,
            timer,
            countdown: None,
            focus_generation: 0,
            focusing: false,
            suspended: false,
            perf: crate::perf::on().then(Default::default),
            granted: false,
            warmed: false,
            open_pending: false,
            last_meta: Metadata::default(),
            locked: false,
            lock_exposure: None,
            device: 0,
            natural_landscape: None,
            display_rotation: 0,
            zoom: 1.0,
            zoom_start: 1.0,
            orientation: crate::device::Orientation::watch({
                let s = sender.clone();
                move |d| s.input(Msg::Orientation(d))
            }),
        };
        if let Some(stats) = &model.perf {
            window.connect_map(|_| perf!("window-mapped"));
            let stats = stats.clone();
            let connected = std::cell::Cell::new(false);
            viewfinder.connect_realize(move |vf| {
                let stats = stats.clone();
                let first = std::cell::Cell::new(true);
                if let Some(clock) = vf.frame_clock().filter(|_| !connected.replace(true)) {
                    clock.connect_after_paint(move |_| {
                        if first.replace(false) {
                            perf!("window-painted");
                        }
                        stats.borrow_mut().painted();
                    });
                }
            });
        }
        let widgets = Widgets {
            window,
            toasts,
            stack,
            status,
            retry,
            open_settings,
            viewfinder,
            grid,
            focus_layer,
            focus_ring,
            countdown,
            controls_title,
            controls_toggle,
            controls_box,
            capture_group,
            mode_row,
            mode_handler,
            raw_row,
            raw_toggle,
            quick,
            resolution,
            resolution_action,
            timer_button,
            timer_label,
            switch,
            capture,
            modes,
            record_pill,
            lock_pill,
            lock_label,
            lock_ev,
            record_time,
            thumbnail,
            gallery,
            saving_spinner,
            gallery_placeholder,
            chips,
        };
        model.show_timer(&widgets);
        if video {
            widgets.modes.set_active_name(Some("video"));
        }
        perf!("app-init-done");
        ComponentParts { model, widgets }
    }

    fn update_cmd_with_view(&mut self, w: &mut Self::Widgets, msg: CmdOut, _sender: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            CmdOut::Access(Access::Denied) => {
                perf!("portal", "denied");
                self.show_status(
                    w,
                    Some("camera-disabled-symbolic"),
                    &gettext("No Camera Access"),
                    Some(&gettext("Allow Obscura to use the camera in Settings › Privacy › Camera, then try again.")),
                    true,
                );
                w.open_settings.set_visible(true);
            }
            CmdOut::Access(access) => {
                perf!("portal", "{access:?}");
                if let Access::Unavailable(why) = &access {
                    log::warn!("camera portal unavailable ({why}); using the cameras directly");
                }
                self.granted = true;
                if !self.cameras.is_empty() {
                    self.show_status(w, None, &gettext("Starting Camera…"), None, false);
                    self.reopen(w, self.camera, None);
                }
            }
        }
    }

    fn update_with_view(&mut self, w: &mut Self::Widgets, msg: Msg, sender: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            Msg::ToggleControls => w.controls_toggle.set_active(!w.controls_toggle.is_active()),
            Msg::ShowControl(names) => {
                w.controls_toggle.set_active(true);
                if let Some(panel) = self.panel.clone() {
                    // Once the sheet or sidebar has mapped the rows.
                    glib::timeout_add_local_once(Duration::from_millis(250), move || panel.focus(names));
                }
            }
            Msg::PortalSlow => {
                // The permission dialog is up: say what it is for.
                if !self.granted && w.stack.visible_child_name().as_deref() == Some("status") && !w.retry.is_visible() {
                    w.status.set_title(&gettext("Camera Access"));
                    w.status.set_description(Some(&gettext("Obscura needs your permission to use the camera")));
                }
            }
            Msg::Retry => {
                self.show_status(w, None, &gettext("Starting Camera…"), None, false);
                if !self.granted {
                    sender.oneshot_command(async { CmdOut::Access(portal::request_access().await) });
                } else {
                    self.reopen(w, self.camera, None);
                }
            }
            Msg::OpenSettings => {
                let args = [std::ffi::OsStr::new("gnome-control-center"), std::ffi::OsStr::new("camera")];
                if let Err(e) = gio::Subprocess::newv(&args, gio::SubprocessFlags::NONE) {
                    w.toasts.add_toast(adw::Toast::new(&format!("{}: {e}", gettext("Could not open Settings"))));
                }
            }
            Msg::Camera(Event::Cameras(cameras)) => {
                if cameras.is_empty() {
                    return self.show_status(
                        w,
                        Some("camera-hardware-disabled-symbolic"),
                        &gettext("No Camera Found"),
                        Some(&gettext("Connect a camera to take pictures and videos.")),
                        false,
                    );
                }
                let wanted = self.settings.as_ref().map(|s| s.string("camera").to_string()).unwrap_or_default();
                let index = cameras
                    .iter()
                    .position(|c| c.id == wanted)
                    .or_else(|| cameras.iter().position(|c| c.facing == Facing::Back))
                    .unwrap_or(0);
                w.switch.set_visible(cameras.len() > 1);
                self.cameras = cameras;
                self.camera = index;
                if self.granted {
                    self.reopen(w, index, None);
                }
            }
            Msg::Camera(Event::Opened(_)) if self.suspended => {
                if let Some(b) = self.backend() {
                    b.send(Cmd::Close);
                }
            }
            Msg::Camera(Event::Opened(session)) => {
                perf!("session-opened", "camera={} mode={}x{}", session.camera, session.mode.width, session.mode.height);
                w.controls_title.set_title(&session.info.name());
                w.controls_title.set_subtitle(if session.info.facing == Facing::External { "" } else { &session.info.model });
                let front = session.info.facing == Facing::Front;
                w.viewfinder.set_rotation(crate::device::upright(session.info.rotation, front, self.display_rotation), front);
                w.stack.set_visible_child_name("camera");
                w.quick.set_visible(true);
                w.capture.set_sensitive(!self.saving);
                w.raw_row.set_visible(session.raw);
                w.raw_toggle.set_visible(session.raw);

                w.mode_row.block_signal(&w.mode_handler);
                let labels: Vec<String> = session.modes.iter().map(mode_label).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                w.mode_row.set_model(Some(&gtk::StringList::new(&labels)));
                w.mode_row.set_selected(session.modes.iter().position(|m| *m == session.mode).unwrap_or(0) as u32);
                w.mode_row.unblock_signal(&w.mode_handler);
                let menu = gio::Menu::new();
                for m in session.modes.iter().filter(|m| !self.video || wide(m)) {
                    let text = if self.video { format!("{} · {} × {}", video_name(m), m.width, m.height) } else { mode_label(m) };
                    menu.append(Some(&text), Some(&format!("app.resolution::{}x{}", m.width, m.height)));
                }
                w.resolution.set_menu_model(Some(&menu));
                w.resolution.set_label(&if self.video { video_name(&session.mode) } else { aspect(&session.mode) });
                w.resolution.set_tooltip_text(Some(&format!("{} ({})", gettext("Resolution"), mode_label(&session.mode))));
                w.resolution_action.set_state(&format!("{}x{}", session.mode.width, session.mode.height).to_variant());

                while let Some(child) = w.controls_box.first_child() {
                    w.controls_box.remove(&child);
                }
                self.panel = None;
                let backend = self.backend.clone();
                let on_change: controls::OnChange = Rc::new(move |id, value| {
                    if let Some(b) = backend.as_deref() {
                        b.send(Cmd::SetControl { id, value });
                    }
                });
                let panel = controls::build(&session.controls, session.fps, &w.capture_group, on_change);
                w.controls_box.append(&panel.widget);
                let has = |names: &[&str]| names.iter().any(|n| panel.has(n));
                let c = &w.chips;
                c.iso.set_visible(has(&["AnalogueGain", "AeEnable"]));
                c.shutter.set_visible(has(&["ExposureTime", "AeEnable"]));
                c.ev.set_visible(has(&["ExposureValue"]));
                c.wb.set_visible(has(&["AwbMode", "AwbEnable", "ColourTemperature"]));
                c.focus.set_visible(has(&["AfMode", "LensPosition"]));
                // Frame rate is a video decision; photos leave it to exposure.
                c.fps.set_visible(self.video && has(&["FrameDurationLimits"]));
                self.panel = Some(panel);
                self.remember(&session);
                self.camera = session.camera;
                self.session = Some(session);
            }
            Msg::Camera(Event::FrameReady) => {
                let started = self.perf.is_some().then(Instant::now);
                let Some(frame) = self.backend.as_deref().and_then(|b| b.take_frame()) else { return };
                let copied = frame.bytes.is_some();
                match viewfinder::texture(frame) {
                    Ok(texture) => w.viewfinder.set_texture(Some(texture)),
                    Err(()) if !copied && !self.copy_frames => {
                        self.copy_frames = true;
                        if let Some(b) = self.backend() {
                            b.send(Cmd::CopyFrames(true));
                        }
                    }
                    Err(()) => {}
                }
                if std::mem::take(&mut self.awaiting_frame) {
                    w.viewfinder.remove_css_class("switching");
                    w.grid.queue_draw();
                    perf!("frame-first-delivered");
                    if !std::mem::replace(&mut self.warmed, true) {
                        std::thread::spawn(crate::photo::warm);
                    }
                }
                if let (Some(p), Some(started)) = (&self.perf, started) {
                    let mut p = p.borrow_mut();
                    let ns = started.elapsed().as_nanos() as u64;
                    p.delivered += 1;
                    p.dropped += self.backend.as_deref().map_or(0, |b| b.take_replaced());
                    p.set_since_paint += 1;
                    p.main_ns += ns;
                    p.main_max_ns = p.main_max_ns.max(ns);
                }
                self.frames += 1;
                let elapsed = self.fps_since.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.fps = self.frames as f64 / elapsed;
                    self.frames = 0;
                    self.fps_since = Instant::now();
                }
            }
            Msg::Camera(Event::Metadata(meta)) => {
                self.frame_duration = meta.get("FrameDuration");
                self.last_meta = meta.clone();
                if let Some(p) = &self.panel
                    && w.controls_toggle.is_active()
                {
                    p.show_metadata(&meta);
                }
                self.update_chips(&w.chips, &meta);
                // ponytail: the display turn is polled at the metadata rate
                // rather than wired to every monitor and window signal.
                self.apply_orientation(w);
                let settled = match meta.get("AfState").map(|s| s as i32) {
                    Some(2) => Some("focused"),
                    Some(3) => Some("failed"),
                    _ => None,
                };
                if self.focusing && let Some(class) = settled {
                    self.focusing = false;
                    w.focus_ring.remove_css_class("scanning");
                    w.focus_ring.add_css_class(class);
                    let (s, generation) = (sender.clone(), self.focus_generation);
                    glib::timeout_add_local_once(Duration::from_millis(900), move || s.input(Msg::HideFocus(generation)));
                }
            }
            Msg::Camera(Event::Still(still)) => {
                if let Some(t) = self.perf.as_ref().and_then(|p| p.borrow().shutter) {
                    perf!("still-received", "since_shutter_ms={:.0}", t.elapsed().as_secs_f64() * 1e3);
                }
                let mut still = still;
                let front = still.info.facing == Facing::Front;
                still.info.rotation = crate::device::upright(still.info.rotation, front, self.device);
                still.zoom = self.zoom;
                let raw = self.raw_enabled();
                let input = sender.input_sender().clone();
                let path = crate::photo::next_path();
                self.last_capture = Some(path.clone());
                std::thread::spawn(move || {
                    let result = crate::photo::save(&still, raw, &path).map_err(|e| e.to_string()).map(|path| (path, thumbnail(&still)));
                    let _ = input.send(Msg::Saved(result));
                });
            }
            Msg::Camera(Event::Error(e)) => {
                log::warn!("{e}");
                if self.session.is_none() {
                    let (title, description) = if e.contains("busy") {
                        (gettext("Camera Busy"), gettext("Another app is using this camera. Close it, then try again."))
                    } else {
                        (gettext("Camera Unavailable"), e.clone())
                    };
                    self.show_status(w, Some("camera-disabled-symbolic"), &title, Some(&description), self.backend.is_some());
                } else {
                    w.toasts.add_toast(adw::Toast::new(&e));
                }
            }
            Msg::SwitchCamera => {
                if self.session.is_some() && self.recorder.is_none() && self.cameras.len() > 1 {
                    let next = (self.camera + 1) % self.cameras.len();
                    if w.switch.has_css_class("flipped") { w.switch.remove_css_class("flipped") } else { w.switch.add_css_class("flipped") }
                    self.reopen(w, next, None);
                }
            }
            Msg::SelectModeIndex(i) => {
                if let Some(mode) = self.session.as_ref().and_then(|s| s.modes.get(i as usize).copied()) {
                    sender.input(Msg::SelectMode(mode));
                }
            }
            Msg::SelectMode(mode) => {
                if let Some(s) = &self.session
                    && s.modes.contains(&mode)
                    && mode != s.mode
                    && self.recorder.is_none()
                {
                    let camera = s.camera;
                    self.reopen(w, camera, Some(mode));
                }
            }
            Msg::Capture => {
                if let Some((_, source)) = self.countdown.take() {
                    source.remove();
                    w.countdown.set_visible(false);
                    return;
                }
                let recording = self.recorder.is_some() || self.record_started.is_some();
                if self.timer > 0 && self.session.is_some() && !recording && !self.saving {
                    let s = sender.clone();
                    let source = glib::timeout_add_seconds_local(1, move || {
                        s.input(Msg::Countdown);
                        glib::ControlFlow::Continue
                    });
                    w.countdown.set_label(&self.timer.to_string());
                    w.countdown.set_visible(true);
                    self.countdown = Some((self.timer, source));
                    return;
                }
                self.shoot(w, &sender);
            }
            Msg::Countdown => {
                if let Some((n, source)) = self.countdown.take() {
                    if n > 1 {
                        w.countdown.set_label(&(n - 1).to_string());
                        self.countdown = Some((n - 1, source));
                    } else {
                        source.remove();
                        w.countdown.set_visible(false);
                        self.shoot(w, &sender);
                    }
                }
            }
            Msg::CycleTimer => {
                self.timer = match self.timer {
                    0 => 3,
                    3 => 10,
                    _ => 0,
                };
                if let Some(s) = &self.settings {
                    let _ = s.set_int("timer", self.timer as i32);
                }
                self.show_timer(w);
            }
            Msg::TapFocus(x, y) => {
                // A tap after a lock lets go of it, and focuses there.
                self.unlock(w);
                let (Some(session), Some(panel)) = (&self.session, &self.panel) else { return };
                let Some(trigger) = session.controls.iter().find(|c| c.name == "AfTrigger") else { return };
                let Some((nx, ny)) = w.viewfinder.to_sensor(x, y) else { return };
                // A tap is one scan: continuous focus would wander off again.
                if panel.has("AfMode") && !panel.select("AfMode", "Auto") {
                    return;
                }
                let Some(b) = self.backend.as_deref() else { return };
                // Without AfWindows the camera focuses where it always does,
                // so the ring goes there rather than pretend.
                let (x, y) = if session.af_windows {
                    b.send(Cmd::FocusAt(nx, ny));
                    (x, y)
                } else {
                    w.viewfinder.layout().map(|l| ((l.x + l.width / 2.0) as f64, (l.y + l.height / 2.0) as f64)).unwrap_or((x, y))
                };
                let start = trigger.enums.iter().find(|(_, n)| n.ends_with("Start")).map(|(v, _)| *v).unwrap_or(0);
                b.send(Cmd::SetControl { id: trigger.id, value: vec![start as f64] });
                let (rw, rh) = (w.focus_ring.width_request() as f64, w.focus_ring.height_request() as f64);
                w.focus_layer.move_(&w.focus_ring, x - rw / 2.0, y - rh / 2.0);
                for class in ["focused", "failed"] {
                    w.focus_ring.remove_css_class(class);
                }
                w.focus_ring.add_css_class("scanning");
                w.focus_ring.set_visible(true);
                self.focusing = true;
                self.focus_generation += 1;
                let (s, generation) = (sender.clone(), self.focus_generation);
                glib::timeout_add_local_once(Duration::from_secs(4), move || s.input(Msg::HideFocus(generation)));
            }
            // A hidden camera app should not keep the sensor, the battery
            // and the camera itself busy.
            Msg::Suspended(true) if self.recorder.is_none() && self.countdown.is_none() && !self.cameras.is_empty() => {
                self.suspended = true;
                self.orientation.claim(false);
                if let Some(b) = self.backend() {
                    b.send(Cmd::Close);
                }
                w.viewfinder.add_css_class("switching");
                w.viewfinder.set_texture(None);
                self.session = None;
                w.capture.set_sensitive(false);
            }
            Msg::Suspended(false) if self.suspended => {
                self.suspended = false;
                self.orientation.claim(true);
                self.reopen(w, self.camera, None);
            }
            Msg::Suspended(_) => {}
            Msg::Lock(x, y) => self.lock(w, x, y),
            Msg::LockExposure(ev) => {
                if let (Some(base), Some(panel)) = (self.lock_exposure, &self.panel) {
                    panel.set("ExposureTime", base * 2f64.powf(ev));
                }
            }
            Msg::Orientation(d) => {
                self.device = d;
                self.apply_orientation(w);
            }
            Msg::ZoomBegin => self.zoom_start = self.zoom,
            Msg::Pinch(scale) => self.set_zoom(w, self.zoom_start * scale),
            Msg::CycleZoom => {
                let next = if self.zoom < 1.99 { 2.0 } else if self.zoom < 3.99 { 4.0 } else { 1.0 };
                self.set_zoom(w, next);
            }
            Msg::Preferences => self.preferences(w),
            Msg::HideFocus(generation) => {
                if generation == self.focus_generation {
                    self.focusing = false;
                    w.focus_ring.set_visible(false);
                }
            }
            Msg::RecordingStarted(result) => {
                w.capture.set_sensitive(true);
                match result {
                    Ok(recorder) => {
                        perf!("recording-started", "encoder={}", recorder.encoder);
                        if let Some(b) = self.backend() {
                            b.send(Cmd::Record(Some(recorder.clone())));
                        }
                        self.recorder = Some(recorder);
                        self.record_started = Some(Instant::now());
                        w.record_time.set_label("0:00");
                        self.set_recording_ui(w, true);
                        let s = sender.clone();
                        self.record_tick = Some(glib::timeout_add_local(Duration::from_millis(500), move || {
                            s.input(Msg::Tick);
                            glib::ControlFlow::Continue
                        }));
                    }
                    Err(e) => {
                        log::warn!("recording did not start: {e}");
                        self.record_started = None;
                        w.toasts.add_toast(adw::Toast::new(&format!("{}: {e}", gettext("Could not record"))));
                    }
                }
            }
            Msg::Tick => {
                if let (Some(t), Some(_)) = (self.record_started, &self.recorder) {
                    let secs = t.elapsed().as_secs();
                    w.record_time.set_label(&format!("{}:{:02}", secs / 60, secs % 60));
                }
            }
            Msg::RecordingDone(result) => {
                self.record_started = None;
                if let Some(source) = self.record_tick.take() {
                    source.remove();
                }
                w.capture.set_sensitive(true);
                self.set_recording_ui(w, false);
                match result {
                    Ok(path) => {
                        perf!("recording-saved");
                        self.last_capture = Some(path);
                        w.gallery.set_sensitive(true);
                        let toast = adw::Toast::builder().title(gettext("Video saved")).button_label(gettext("_Open")).action_name("app.open-last").build();
                        w.toasts.add_toast(toast);
                    }
                    Err(e) => {
                        log::warn!("recording failed: {e}");
                        w.toasts.add_toast(adw::Toast::new(&format!("{}: {e}", gettext("Could not save video"))));
                    }
                }
            }
            Msg::SetVideo(video) => {
                perf!("mode", "video={video}");
                if video {
                    // Probe encoders while the camera reopens, not at the first recording.
                    std::thread::spawn(crate::video::encoder);
                }
                self.video = video;
                if let Some(s) = &self.settings {
                    let _ = s.set_boolean("video", video);
                }
                if let Some(a) = relm4::main_application().lookup_action("mode") {
                    a.change_state(&(if video { "video" } else { "photo" }).to_variant());
                }
                let tip = if video { gettext("Start Recording") } else { gettext("Take Photo") };
                w.capture.set_icon_name(if video { "media-record-symbolic" } else { "camera-photo-symbolic" });
                w.capture.set_tooltip_text(Some(&tip));
                label(&w.capture, &tip);
                if video { w.capture.add_css_class("video") } else { w.capture.remove_css_class("video") }
                w.chips.fps.set_visible(video && self.panel.as_ref().is_some_and(|p| p.has("FrameDurationLimits")));
                // ponytail: zoom crops photos only; recordings stay uncropped.
                w.chips.zoom.set_visible(!video);
                if video {
                    self.set_zoom(w, 1.0);
                }
                // Video wants a 16:9 mode, photos the full sensor.
                if let Some(s) = &self.session {
                    // Video opens at 1080p (a fast, binned mode where there is
                    // one) and photos at the full sensor; the menu offers the rest.
                    let target = if video {
                        Some(s.modes.iter().copied().filter(|m| wide(m) && m.width >= 1920).min_by_key(|m| m.width).or_else(|| s.modes.iter().copied().filter(wide).max_by_key(|m| m.width)))
                    } else {
                        Some(s.modes.first().copied())
                    };
                    if let Some(Some(mode)) = target {
                        let camera = s.camera;
                        self.reopen(w, camera, Some(mode));
                    }
                }
            }
            Msg::Found(path, thumb) => {
                if self.last_capture.is_none() {
                    self.show_thumbnail(w, path, thumb);
                }
            }
            Msg::Saved(result) => {
                w.saving_spinner.set_visible(false);
                self.saving = false;
                w.capture.set_sensitive(self.session.is_some());
                match result {
                    Ok((path, thumb)) => {
                        self.show_thumbnail(w, path, thumb);
                        if let Some(t) = self.perf.as_ref().and_then(|p| p.borrow_mut().shutter.take()) {
                            perf!("thumbnail-shown", "since_shutter_ms={:.0}", t.elapsed().as_secs_f64() * 1e3);
                        }
                        if std::mem::take(&mut self.open_pending) {
                            sender.input(Msg::OpenLast);
                        }
                    }
                    Err(e) => w.toasts.add_toast(adw::Toast::new(&format!("{}: {e}", gettext("Could not save photo")))),
                }
            }
            Msg::OpenLast if self.saving => self.open_pending = true,
            Msg::OpenLast => {
                if let Some(path) = &self.last_capture {
                    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(Some(&w.window), None::<&gio::Cancellable>, |_| {});
                }
            }
        }
    }
}

impl App {
    fn lock(&mut self, w: &Widgets, x: f64, y: f64) {
        let Some(panel) = self.panel.clone() else { return };
        if w.viewfinder.to_sensor(x, y).is_none() {
            return;
        }
        let meta = &self.last_meta;
        let mut held = Vec::new();
        // Hold what the camera chose last, as manual values in the panel, so
        // every control shows the lock too.
        if let (Some(e), Some(g)) = (meta.get("ExposureTime"), meta.get("AnalogueGain"))
            && panel.has("ExposureTime")
        {
            if !(panel.select("ExposureTimeMode", "Manual") | panel.select("AnalogueGainMode", "Manual")) {
                panel.set("AeEnable", 0.0);
            }
            panel.set("ExposureTime", e);
            panel.set("AnalogueGain", g);
            self.lock_exposure = Some(e);
            held.push("AE");
        }
        if let Some(d) = meta.get("LensPosition")
            && panel.has("LensPosition")
            && panel.select("AfMode", "Manual")
        {
            panel.set("LensPosition", d);
            held.push("AF");
        }
        if held.is_empty() {
            return;
        }
        self.locked = true;
        w.lock_label.set_label(&format!("{} {}", held.join("/"), gettext("LOCK")));
        w.lock_ev.set_value(0.0);
        w.lock_ev.set_visible(self.lock_exposure.is_some());
        w.lock_pill.set_visible(true);
        let (rw, rh) = (w.focus_ring.width_request() as f64, w.focus_ring.height_request() as f64);
        w.focus_layer.move_(&w.focus_ring, x - rw / 2.0, y - rh / 2.0);
        w.focus_ring.remove_css_class("scanning");
        w.focus_ring.remove_css_class("failed");
        w.focus_ring.add_css_class("focused");
        w.focus_ring.set_visible(true);
        self.focusing = false;
        self.focus_generation += 1;
        self.feedback_event("camera-focus");
    }

    fn unlock(&mut self, w: &Widgets) {
        if !std::mem::take(&mut self.locked) {
            return;
        }
        self.lock_exposure = None;
        w.lock_pill.set_visible(false);
        w.focus_ring.set_visible(false);
        if let Some(panel) = &self.panel {
            if !(panel.select("ExposureTimeMode", "Auto") | panel.select("AnalogueGainMode", "Auto")) {
                panel.set("AeEnable", 1.0);
            }
            panel.select("AfMode", "Continuous");
        }
    }

    fn feedback_event(&self, event: &str) {
        if self.settings.as_ref().is_none_or(|s| s.boolean("shutter-sound")) {
            crate::device::feedback(event);
        }
    }

    fn feedback(&self) {
        self.feedback_event("camera-shutter");
    }

    fn set_zoom(&mut self, w: &Widgets, zoom: f64) {
        self.zoom = zoom.clamp(1.0, 4.0);
        w.viewfinder.set_zoom(self.zoom as f32);
        let text = if (self.zoom - self.zoom.round()).abs() < 0.05 { format!("{:.0}×", self.zoom) } else { format!("{:.1}×", self.zoom) };
        set_chip(&w.chips.zoom, &text, self.zoom > 1.0);
    }

    /// Turn the viewfinder for the display: when the compositor has rotated
    /// the screen to follow the device, the picture must turn with it.
    fn apply_orientation(&mut self, w: &Widgets) {
        let monitor = w.window.surface().and_then(|s| WidgetExt::display(&w.window).monitor_at_surface(&s));
        let Some(geometry) = monitor.map(|m| m.geometry()) else { return };
        let landscape = geometry.width() > geometry.height();
        if self.device % 180 == 0 {
            self.natural_landscape = Some(landscape);
        }
        // Until the device has been seen upright, assume a phone: portrait.
        let rotated = landscape != self.natural_landscape.unwrap_or(false);
        let display = if rotated && self.device % 180 != 0 { self.device } else { 0 };
        if display == self.display_rotation {
            return;
        }
        self.display_rotation = display;
        if let Some(session) = &self.session {
            let front = session.info.facing == Facing::Front;
            w.viewfinder.set_rotation(crate::device::upright(session.info.rotation, front, display), front);
            w.grid.queue_draw();
        }
    }

    fn preferences(&self, w: &Widgets) {
        let Some(settings) = &self.settings else { return };
        let switch = |key: &str, title: &str, subtitle: &str| {
            let row = adw::SwitchRow::builder().title(title).subtitle(subtitle).build();
            settings.bind(key, &row, "active").build();
            row
        };
        let capture = adw::PreferencesGroup::builder().title(gettext("Capture")).build();
        capture.add(&switch("shutter-sound", &gettext("Shutter Sound"), &gettext("Play a sound or vibrate when taking pictures, as the device's feedback settings allow")));
        capture.add(&switch("full-resolution", &gettext("Full-Resolution Photos"), &gettext("The camera switches to its full photo size for a moment; turn off for photos straight from the viewfinder, with no wait")));
        capture.add(&switch("raw", &gettext("Save RAW"), &gettext("Also write a DNG next to each photo, on cameras that provide raw images")));
        let viewfinder = adw::PreferencesGroup::builder().title(gettext("Viewfinder")).build();
        viewfinder.add(&switch("grid", &gettext("Grid"), &gettext("Rule-of-thirds lines to help composition")));
        viewfinder.add(&switch("show-info", &gettext("Capture Info"), &gettext("ISO, shutter speed, white balance and focus over the picture")));
        let page = adw::PreferencesPage::new();
        page.add(&capture);
        page.add(&viewfinder);
        let dialog = adw::PreferencesDialog::builder().title(gettext("Preferences")).build();
        dialog.add(&page);
        dialog.present(Some(&w.window));
    }

    fn show_thumbnail(&mut self, w: &Widgets, path: PathBuf, thumb: Thumb) {
        let bytes = glib::Bytes::from_owned(thumb.rgba);
        let tex = gdk::MemoryTexture::new(thumb.width as i32, thumb.height as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, thumb.width as usize * 4);
        w.thumbnail.set_texture(Some(tex.upcast()));
        w.thumbnail.set_rotation(thumb.rotation, false);
        w.gallery_placeholder.set_visible(false);
        w.gallery.set_sensitive(true);
        self.last_capture = Some(path);
    }
}

/// A small RGBA thumbnail by point sampling the still.
fn thumbnail(still: &Still) -> Thumb {
    let (tw, th) = (128u32, (128 * still.height / still.width.max(1)).max(1));
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    let bgr = matches!(&still.fourcc.to_le_bytes(), b"XR24" | b"AR24");
    for y in 0..th {
        for x in 0..tw {
            let sx = (x * still.width / tw) as usize;
            let sy = (y * still.height / th) as usize;
            let i = sy * still.stride as usize + sx * 4;
            let px = still.rgba.get(i..i + 3).unwrap_or(&[0, 0, 0]);
            if bgr {
                rgba.extend([px[2], px[1], px[0], 255]);
            } else {
                rgba.extend([px[0], px[1], px[2], 255]);
            }
        }
    }
    Thumb { rgba, width: tw, height: th, rotation: still.info.rotation }
}
