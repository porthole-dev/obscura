// SPDX-License-Identifier: GPL-3.0-or-later

#[macro_use]
mod perf;
mod app;
mod camera;
mod controls;
mod device;
mod dng;
mod photo;
mod portal;
#[cfg(feature = "preview")]
mod preview;
mod video;
mod viewfinder;

pub const APP_ID: &str = "io.github.jertlok.Obscura";
const GETTEXT_PACKAGE: &str = "obscura";

const CSS: &str = "
.camera { background-color: black; }
viewfinder.viewfinder { transition: opacity 250ms ease-out, filter 250ms ease-out; }
viewfinder.viewfinder.flash { opacity: 0.1; transition: none; }
viewfinder.viewfinder.switching { filter: blur(12px); opacity: 0.6; transition: none; }
.capture-bar-side { padding: 12px 16px; background: linear-gradient(to left, alpha(black, 0.65), alpha(black, 0.3) 75%, transparent); }
.capture-bar { padding: 10px 18px 20px 18px; background: linear-gradient(to top, alpha(black, 0.65), alpha(black, 0.35) 75%, transparent); }
.shutter { min-width: 76px; min-height: 76px; padding: 0; border-radius: 9999px; -gtk-icon-size: 28px; background-color: white; color: black; box-shadow: 0 0 0 4px alpha(white, 0.3); transition: transform 120ms ease-out, background-color 200ms, box-shadow 200ms; }
.shutter:hover { background-color: alpha(white, 0.9); }
.shutter:active { transform: scale(0.88); box-shadow: 0 0 0 8px alpha(white, 0.2); }
.shutter:disabled { background-color: alpha(white, 0.45); }
.shutter.video { color: #e01b24; -gtk-icon-size: 36px; }
.shutter.recording { background-color: #e01b24; color: white; -gtk-icon-size: 24px; }
.round-button { min-width: 52px; min-height: 52px; padding: 0; border-radius: 9999px; background-color: alpha(white, 0.14); color: white; }
.round-button:hover { background-color: alpha(white, 0.24); }
.switch-camera image { transition: transform 350ms ease-in-out; }
.switch-camera.flipped image { transform: rotate(180deg); }
.gallery { padding: 2px; }
.gallery viewfinder { border-radius: 9999px; }
.gallery.new { animation: pop 350ms ease-out; }
@keyframes pop { 0% { transform: scale(0.5); } 70% { transform: scale(1.1); } 100% { transform: scale(1); } }
.chip { min-height: 36px; padding: 0 7px; border-radius: 9999px; background-color: alpha(black, 0.45); color: white; font-size: 0.9em; font-weight: 600; }
.chip:hover { background-color: alpha(black, 0.65); }
.chip.manual { color: #f6d32d; }
.mode-switch { background-color: alpha(black, 0.45); }
.raw-toggle { font-weight: 800; font-size: 0.85em; }
.timer.on { color: #f6d32d; }
.lock-pill { background-color: alpha(black, 0.55); color: #f6d32d; border-radius: 9999px; padding: 2px 14px; font-weight: 800; font-size: 0.85em; }
.recording-pill { background-color: alpha(black, 0.55); color: white; border-radius: 9999px; padding: 4px 12px; font-weight: 700; }
.rec-dot { min-width: 10px; min-height: 10px; border-radius: 9999px; background-color: #e01b24; animation: pulse 900ms ease-in-out infinite alternate; }
@keyframes pulse { from { opacity: 1; } to { opacity: 0.2; } }
.focus-ring { border: 2px solid white; border-radius: 9999px; box-shadow: 0 0 6px alpha(black, 0.5); transition: transform 250ms ease-out, border-color 150ms; }
.focus-ring.scanning { transform: scale(0.8); }
.focus-ring.focused { border-color: #f6d32d; }
.focus-ring.failed { border-color: #e01b24; }
.countdown { font-size: 120px; font-weight: 800; color: white; text-shadow: 0 2px 16px alpha(black, 0.7); }
";

fn main() {
    perf::init();
    env_logger::init();
    // SAFETY: called first thing, before any other thread exists.
    unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") };
    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    let _ = gettextrs::bindtextdomain(GETTEXT_PACKAGE, localedir);
    let _ = gettextrs::bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8");
    let _ = gettextrs::textdomain(GETTEXT_PACKAGE);

    let app = relm4::RelmApp::new(APP_ID);
    perf!("gtk-init");
    relm4::set_global_css(CSS);
    perf!("css");
    app.run::<app::App>(());
}
