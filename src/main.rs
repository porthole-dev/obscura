// SPDX-License-Identifier: GPL-3.0-or-later

mod app;
mod camera;
mod controls;
mod dng;
mod photo;
mod portal;
mod viewfinder;

pub const APP_ID: &str = "io.github.jertlok.Obscura";
const GETTEXT_PACKAGE: &str = "obscura";

const CSS: &str = "
.viewfinder { background-color: black; }
.viewfinder.flash { opacity: 0.2; }
.capture-bar { padding: 18px 24px; background: linear-gradient(to top, alpha(black, 0.55), transparent); }
.shutter { min-width: 72px; min-height: 72px; -gtk-icon-size: 28px; background-color: white; color: black; border: 4px solid alpha(white, 0.5); background-clip: padding-box; }
.shutter:disabled { background-color: alpha(white, 0.5); }
.bottom-button { min-width: 52px; min-height: 52px; padding: 0; }
.gallery viewfinder { border-radius: 9999px; min-width: 48px; min-height: 48px; }
.capture-info { background-color: alpha(black, 0.55); color: white; border-radius: 9999px; padding: 4px 12px; }
";

fn main() {
    env_logger::init();
    // SAFETY: called first thing, before any other thread exists.
    unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") };
    let localedir = option_env!("LOCALEDIR").unwrap_or("/usr/share/locale");
    let _ = gettextrs::bindtextdomain(GETTEXT_PACKAGE, localedir);
    let _ = gettextrs::bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8");
    let _ = gettextrs::textdomain(GETTEXT_PACKAGE);
    gst::init().expect("GStreamer");

    let app = relm4::RelmApp::new(APP_ID);
    relm4::set_global_css(CSS);
    app.run::<app::App>(());
}
