// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gettextrs::gettext;
use relm4::adw::{self, prelude::*};
use relm4::gtk::{self, gdk, gio, glib};
use relm4::{Component, ComponentParts, ComponentSender};

use crate::camera::{Backend, CameraInfo, Cmd, Event, Facing, LibcameraBackend, Metadata, Mode, Session, Still};
use crate::controls::{self, Panel};
use crate::portal::{self, Access};
use crate::viewfinder::{self, Viewfinder};
use crate::APP_ID;

pub struct App {
    backend: Option<Rc<LibcameraBackend>>,
    cameras: Vec<CameraInfo>,
    session: Option<Session>,
    panel: Option<Rc<Panel>>,
    copy_frames: bool,
    frames: u32,
    fps_since: Instant,
    fps: f64,
    last_capture: Option<PathBuf>,
    settings: Option<gio::Settings>,
    saving: bool,
}

#[derive(Debug)]
pub enum Msg {
    Camera(Event),
    SwitchCamera,
    SelectMode(u32),
    Capture,
    OpenLast,
    Retry,
    ToggleSidebar,
    Saved(Result<(PathBuf, Vec<u8>, u32, u32, i32), String>),
}

#[derive(Debug)]
pub enum CmdOut {
    Access(Access),
}

pub struct Widgets {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    status: adw::StatusPage,
    retry: gtk::Button,
    viewfinder: Viewfinder,
    info: gtk::Label,
    split: adw::OverlaySplitView,
    controls_box: gtk::Box,
    mode_row: adw::ComboRow,
    mode_handler: glib::SignalHandlerId,
    raw_row: adw::SwitchRow,
    switch: gtk::Button,
    capture: gtk::Button,
    thumbnail: Viewfinder,
    gallery: gtk::Button,
}

fn mode_label(m: &Mode) -> String {
    let mp = m.width as f64 * m.height as f64 / 1e6;
    let g = gcd(m.width, m.height).max(1);
    let (aw, ah) = (m.width / g, m.height / g);
    let aspect = match (aw, ah) {
        (4, 3) | (16, 9) | (3, 2) | (1, 1) => format!("{aw}:{ah}"),
        _ if (m.width as f64 / m.height as f64 - 4.0 / 3.0).abs() < 0.02 => "4:3".into(),
        _ if (m.width as f64 / m.height as f64 - 16.0 / 9.0).abs() < 0.03 => "16:9".into(),
        _ => format!("{:.2}:1", m.width as f64 / m.height as f64),
    };
    let _ = mp;
    format!("{} × {} ({aspect})", m.width, m.height)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
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

    fn open(&self, index: usize, mode: Option<Mode>) {
        let mode = mode.or_else(|| self.cameras.get(index).and_then(|c| self.saved_mode(&c.id)));
        if let Some(b) = self.backend() {
            b.send(Cmd::Open { camera: index, mode });
        }
    }

    fn show_status(&self, w: &Widgets, icon: &str, title: &str, description: &str, retry: bool) {
        w.status.set_icon_name(Some(icon));
        w.status.set_title(title);
        w.status.set_description(Some(description));
        w.retry.set_visible(retry);
        w.stack.set_visible_child_name("status");
    }
}

fn info_line(meta: &Metadata, fps: f64) -> String {
    let mut parts = Vec::new();
    if let Some(us) = meta.get("ExposureTime") {
        let s = us / 1e6;
        parts.push(if s >= 0.5 { format!("{s:.1} s") } else { format!("1/{:.0} s", 1.0 / s.max(1e-6)) });
    }
    if let Some(g) = meta.get("AnalogueGain") {
        parts.push(format!("ISO {:.0}", g * meta.get("DigitalGain").unwrap_or(1.0) * 100.0));
    }
    if let Some(k) = meta.get("ColourTemperature") {
        parts.push(format!("{k:.0} K"));
    }
    if let Some(l) = meta.get("LensPosition") {
        parts.push(format!("{} {l:.1}", gettext("Focus")));
    }
    if let Some(s) = meta.get("AfState") {
        let state = match s as i32 {
            1 => gettext("focusing"),
            2 => gettext("focused"),
            3 => gettext("focus failed"),
            _ => String::new(),
        };
        if !state.is_empty() {
            parts.push(state);
        }
    }
    parts.push(format!("{fps:.0} fps"));
    parts.join("  ·  ")
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
            .default_width(900)
            .default_height(640)
            .width_request(360)
            .height_request(294)
            .build()
    }

    fn init(_: (), window: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let settings = gio::SettingsSchemaSource::default()
            .and_then(|src| src.lookup(APP_ID, true))
            .map(|_| gio::Settings::new(APP_ID));

        // Header
        let title = adw::WindowTitle::new(&gettext("Obscura"), "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let sidebar_toggle = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-right-symbolic")
            .tooltip_text(gettext("Camera Controls"))
            .build();
        header.pack_end(&sidebar_toggle);
        let menu = gio::Menu::new();
        menu.append(Some(&gettext("Show Capture _Info")), Some("app.show-info"));
        menu.append(Some(&gettext("_Keyboard Shortcuts")), Some("win.show-help-overlay"));
        menu.append(Some(&gettext("_About Obscura")), Some("app.about"));
        header.pack_end(&gtk::MenuButton::builder().icon_name("open-menu-symbolic").menu_model(&menu).primary(true).build());

        // Viewfinder page
        let viewfinder = Viewfinder::default();
        viewfinder.set_hexpand(true);
        viewfinder.set_vexpand(true);
        viewfinder.add_css_class("viewfinder");

        let info = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .css_classes(["capture-info", "caption", "numeric"])
            .visible(false)
            .build();

        let capture = gtk::Button::builder()
            .icon_name("camera-photo-symbolic")
            .tooltip_text(gettext("Take Photo"))
            .css_classes(["circular", "shutter"])
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .sensitive(false)
            .build();
        let switch = gtk::Button::builder()
            .icon_name("camera-switch-symbolic")
            .tooltip_text(gettext("Switch Camera"))
            .css_classes(["circular", "osd", "bottom-button"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let thumbnail = Viewfinder::default();
        thumbnail.set_size_request(48, 48);
        thumbnail.set_cover(true);
        thumbnail.set_overflow(gtk::Overflow::Hidden);
        let gallery = gtk::Button::builder()
            .child(&thumbnail)
            .tooltip_text(gettext("Open Last Capture"))
            .css_classes(["circular", "osd", "bottom-button", "gallery"])
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        let bar = gtk::CenterBox::builder()
            .valign(gtk::Align::End)
            .css_classes(["capture-bar"])
            .start_widget(&gallery)
            .center_widget(&capture)
            .end_widget(&switch)
            .build();

        let overlay = gtk::Overlay::builder().child(&viewfinder).build();
        overlay.add_overlay(&info);
        overlay.add_overlay(&bar);

        // Status page (permission, no camera, errors)
        let retry = gtk::Button::builder()
            .label(gettext("Try Again"))
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .visible(false)
            .build();
        let status = adw::StatusPage::builder()
            .icon_name("camera-web-symbolic")
            .title(gettext("Starting Camera…"))
            .child(&retry)
            .build();

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&overlay, Some("camera"));

        // Controls sidebar
        let mode_row = adw::ComboRow::builder().title(gettext("Resolution")).build();
        let raw_row = adw::SwitchRow::builder()
            .title(gettext("Save RAW"))
            .subtitle(gettext("Also write a DNG next to each photo"))
            .visible(false)
            .build();
        if let Some(s) = &settings {
            s.bind("raw", &raw_row, "active").build();
        }
        let capture_group = adw::PreferencesGroup::builder().title(gettext("Capture")).build();
        capture_group.add(&mode_row);
        capture_group.add(&raw_row);
        let controls_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let sidebar_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        sidebar_content.append(&capture_group);
        sidebar_content.append(&controls_box);
        let sidebar = adw::ToolbarView::new();
        sidebar.add_top_bar(&adw::HeaderBar::builder().show_title(false).show_end_title_buttons(false).build());
        sidebar.set_content(Some(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&adw::Clamp::builder().maximum_size(480).child(&sidebar_content).build())
                .build(),
        ));

        let split = adw::OverlaySplitView::builder()
            .sidebar_position(gtk::PackType::End)
            .sidebar(&sidebar)
            .content(&stack)
            .show_sidebar(false)
            .min_sidebar_width(320.0)
            .max_sidebar_width(420.0)
            .build();
        split.bind_property("show-sidebar", &sidebar_toggle, "active").bidirectional().sync_create().build();

        let toolbar = adw::ToolbarView::builder().content(&split).build();
        toolbar.add_top_bar(&header);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));
        window.set_content(Some(&toasts));

        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            720.0,
            adw::LengthUnit::Sp,
        ));
        breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
        window.add_breakpoint(breakpoint);

        // Actions
        let app = relm4::main_application();
        let show_info = gio::SimpleAction::new_stateful("show-info", None, &false.to_variant());
        if let Some(s) = &settings {
            show_info.set_state(&s.boolean("show-info").to_variant());
        }
        info.set_visible(show_info.state().and_then(|v| v.get::<bool>()).unwrap_or(false));
        {
            let info = info.clone();
            let settings = settings.clone();
            show_info.connect_activate(move |a, _| {
                let on = !a.state().and_then(|v| v.get::<bool>()).unwrap_or(false);
                a.set_state(&on.to_variant());
                info.set_visible(on);
                if let Some(s) = &settings {
                    let _ = s.set_boolean("show-info", on);
                }
            });
        }
        app.add_action(&show_info);
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
        app.set_accels_for_action("app.show-info", &["i"]);

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
        }
        let mode_handler = {
            let s = sender.clone();
            mode_row.connect_selected_notify(move |r| s.input(Msg::SelectMode(r.selected())))
        };
        // Everything the capture bar does is also an action: keyboard
        // accelerators, and scriptable over D-Bus (org.gtk.Actions).
        for (name, accels, msg) in [
            ("capture", &["space", "Return"][..], (|| Msg::Capture) as fn() -> Msg),
            ("toggle-controls", &["F9"][..], || Msg::ToggleSidebar),
            ("switch-camera", &["<Ctrl>Tab"][..], || Msg::SwitchCamera),
            ("open-last", &["<Ctrl>o"][..], || Msg::OpenLast),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let s = sender.clone();
            action.connect_activate(move |_, _| s.input(msg()));
            app.add_action(&action);
            app.set_accels_for_action(&format!("app.{name}"), accels);
        }

        sender.oneshot_command(async { CmdOut::Access(portal::request_access().await) });

        let model = App {
            backend: None,
            cameras: Vec::new(),
            session: None,
            panel: None,
            copy_frames: false,
            frames: 0,
            fps_since: Instant::now(),
            fps: 0.0,
            last_capture: None,
            settings,
            saving: false,
        };
        let widgets = Widgets {
            window,
            toasts,
            title,
            stack,
            status,
            retry,
            viewfinder,
            info,
            split,
            controls_box,
            mode_row,
            mode_handler,
            raw_row,
            switch,
            capture,
            thumbnail,
            gallery,
        };
        ComponentParts { model, widgets }
    }

    fn update_cmd_with_view(&mut self, w: &mut Self::Widgets, msg: CmdOut, sender: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            CmdOut::Access(Access::Denied) => self.show_status(
                w,
                "camera-disabled-symbolic",
                &gettext("No Camera Access"),
                &gettext("Allow camera access for this app in Settings › Privacy › Camera, then try again."),
                true,
            ),
            CmdOut::Access(access) => {
                if let Access::Unavailable(why) = &access {
                    log::warn!("camera portal unavailable ({why}); using the cameras directly");
                }
                let input = sender.input_sender().clone();
                self.backend = Some(Rc::new(LibcameraBackend::spawn(move |ev| {
                    let _ = input.send(Msg::Camera(ev));
                })));
                w.status.set_title(&gettext("Starting Camera…"));
            }
        }
    }

    fn update_with_view(&mut self, w: &mut Self::Widgets, msg: Msg, sender: ComponentSender<Self>, _: &Self::Root) {
        match msg {
            Msg::ToggleSidebar => w.split.set_show_sidebar(!w.split.shows_sidebar()),
            Msg::Retry => {
                w.retry.set_visible(false);
                w.status.set_title(&gettext("Starting Camera…"));
                w.status.set_description(None);
                sender.oneshot_command(async { CmdOut::Access(portal::request_access().await) });
            }
            Msg::Camera(Event::Cameras(cameras)) => {
                if cameras.is_empty() {
                    return self.show_status(
                        w,
                        "camera-hardware-disabled-symbolic",
                        &gettext("No Camera Found"),
                        &gettext("Connect a camera to take pictures and videos."),
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
                self.open(index, None);
            }
            Msg::Camera(Event::Opened(session)) => {
                w.title.set_subtitle(&session.info.model);
                w.viewfinder.set_rotation(session.info.rotation, session.info.facing == Facing::Front);
                w.stack.set_visible_child_name("camera");
                w.capture.set_sensitive(true);
                w.raw_row.set_visible(session.raw);

                w.mode_row.block_signal(&w.mode_handler);
                let labels: Vec<String> = session.modes.iter().map(mode_label).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                w.mode_row.set_model(Some(&gtk::StringList::new(&labels)));
                w.mode_row.set_selected(session.modes.iter().position(|m| *m == session.mode).unwrap_or(0) as u32);
                w.mode_row.unblock_signal(&w.mode_handler);

                while let Some(child) = w.controls_box.first_child() {
                    w.controls_box.remove(&child);
                }
                let backend = self.backend.clone();
                let on_change: controls::OnChange = Rc::new(move |id, value| {
                    if let Some(b) = backend.as_deref() {
                        b.send(Cmd::SetControl { id, value });
                    }
                });
                let panel = controls::build(&session.controls, session.fps, on_change);
                w.controls_box.append(&panel.widget);
                self.panel = Some(panel);
                self.remember(&session);
                self.session = Some(session);
            }
            Msg::Camera(Event::Frame(frame)) => {
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
                self.frames += 1;
                let elapsed = self.fps_since.elapsed().as_secs_f64();
                if elapsed >= 1.0 {
                    self.fps = self.frames as f64 / elapsed;
                    self.frames = 0;
                    self.fps_since = Instant::now();
                }
            }
            Msg::Camera(Event::Metadata(meta)) => {
                if let Some(p) = &self.panel {
                    p.show_metadata(&meta);
                }
                w.info.set_label(&info_line(&meta, self.fps));
            }
            Msg::Camera(Event::Still(still)) => {
                let raw = w.raw_row.is_active();
                let input = sender.input_sender().clone();
                std::thread::spawn(move || {
                    let result = crate::photo::save(&still, raw).map_err(|e| e.to_string()).map(|path| {
                        let (thumb, tw, th) = thumbnail(&still);
                        (path, thumb, tw, th, still.info.rotation)
                    });
                    let _ = input.send(Msg::Saved(result));
                });
            }
            Msg::Camera(Event::Error(e)) => {
                log::warn!("{e}");
                if self.session.is_none() {
                    self.show_status(w, "dialog-error-symbolic", &gettext("Camera Error"), &e, true);
                } else {
                    w.toasts.add_toast(adw::Toast::new(&e));
                }
            }
            Msg::SwitchCamera => {
                if let Some(s) = &self.session {
                    let next = (s.camera + 1) % self.cameras.len().max(1);
                    w.capture.set_sensitive(false);
                    w.viewfinder.set_texture(None);
                    self.session = None;
                    self.open(next, None);
                }
            }
            Msg::SelectMode(i) => {
                if let Some(s) = &self.session
                    && let Some(mode) = s.modes.get(i as usize).copied()
                    && mode != s.mode
                {
                    w.capture.set_sensitive(false);
                    w.viewfinder.set_texture(None);
                    let camera = s.camera;
                    self.session = None;
                    self.open(camera, Some(mode));
                }
            }
            Msg::Capture => {
                if self.session.is_some() && !self.saving {
                    self.saving = true;
                    w.capture.set_sensitive(false);
                    w.viewfinder.add_css_class("flash");
                    let vf = w.viewfinder.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || vf.remove_css_class("flash"));
                    if let Some(b) = self.backend() {
                        b.send(Cmd::Capture);
                    }
                }
            }
            Msg::Saved(result) => {
                self.saving = false;
                w.capture.set_sensitive(self.session.is_some());
                match result {
                    Ok((path, thumb, tw, th, rotation)) => {
                        let bytes = glib::Bytes::from_owned(thumb);
                        let tex = gdk::MemoryTexture::new(tw as i32, th as i32, gdk::MemoryFormat::R8g8b8, &bytes, tw as usize * 3);
                        w.thumbnail.set_texture(Some(tex.upcast()));
                        w.thumbnail.set_rotation(rotation, false);
                        w.gallery.set_sensitive(true);
                        self.last_capture = Some(path);
                    }
                    Err(e) => w.toasts.add_toast(adw::Toast::new(&format!("{}: {e}", gettext("Could not save photo")))),
                }
            }
            Msg::OpenLast => {
                if let Some(path) = &self.last_capture {
                    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(
                        Some(&w.window),
                        None::<&gio::Cancellable>,
                        |_| {},
                    );
                }
            }
        }
    }
}

/// A small RGB thumbnail by point sampling the still.
fn thumbnail(still: &Still) -> (Vec<u8>, u32, u32) {
    let (tw, th) = (96u32, (96 * still.height / still.width.max(1)).max(1));
    let mut out = Vec::with_capacity((tw * th * 3) as usize);
    let bgr = matches!(&still.fourcc.to_le_bytes(), b"XR24" | b"AR24");
    for y in 0..th {
        for x in 0..tw {
            let sx = (x * still.width / tw) as usize;
            let sy = (y * still.height / th) as usize;
            let i = sy * still.stride as usize + sx * 4;
            let px = still.rgba.get(i..i + 3).unwrap_or(&[0, 0, 0]);
            if bgr {
                out.extend([px[2], px[1], px[0]]);
            } else {
                out.extend_from_slice(px);
            }
        }
    }
    (out, tw, th)
}
