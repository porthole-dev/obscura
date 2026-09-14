// SPDX-License-Identifier: GPL-3.0-or-later
//! The controls panel, generated from whatever the camera reports. Known
//! libcamera controls get proper names, units and grouping; anything else
//! still gets a widget chosen from its type and range.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gettextrs::gettext;
use relm4::adw::{self, prelude::*};
use relm4::gtk;

use crate::camera::{ControlDesc, Kind, Metadata};

pub type OnChange = Rc<dyn Fn(u32, Vec<f64>)>;
type Reset = Rc<dyn Fn()>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Group {
    Capture,
    Exposure,
    Focus,
    WhiteBalance,
    Image,
    Other,
}

/// Controls that are actions rather than settings: tapping the viewfinder
/// drives them.
const HIDDEN: &[&str] = &["AfTrigger", "AfPause", "AfWindows", "AfMetering"];

/// Known controls in the order a photographer reads them: the automatic
/// switch first, then what it governs.
const KNOWN: &[(&str, Group)] = &[
    ("AeEnable", Group::Exposure),
    ("ExposureValue", Group::Exposure),
    ("AnalogueGainMode", Group::Exposure),
    ("AnalogueGain", Group::Exposure),
    ("ExposureTimeMode", Group::Exposure),
    ("ExposureTime", Group::Exposure),
    ("DigitalGain", Group::Exposure),
    ("AeExposureMode", Group::Exposure),
    ("AeConstraintMode", Group::Exposure),
    ("AeMeteringMode", Group::Exposure),
    ("AeFlickerMode", Group::Exposure),
    ("AeFlickerPeriod", Group::Exposure),
    ("FrameDurationLimits", Group::Capture),
    ("AfMode", Group::Focus),
    ("LensPosition", Group::Focus),
    ("AfRange", Group::Focus),
    ("AfSpeed", Group::Focus),
    ("AwbEnable", Group::WhiteBalance),
    ("AwbMode", Group::WhiteBalance),
    ("ColourTemperature", Group::WhiteBalance),
    ("ColourGains", Group::WhiteBalance),
    ("Brightness", Group::Image),
    ("Contrast", Group::Image),
    ("Saturation", Group::Image),
    ("Sharpness", Group::Image),
    ("Gamma", Group::Image),
    ("NoiseReductionMode", Group::Image),
    ("HdrMode", Group::Image),
];

/// Title, group and sort rank; unknown controls go last, by name.
fn known(name: &str) -> (String, Group, usize) {
    let title = match name {
        "AeEnable" => gettext("Automatic Exposure"),
        "ExposureValue" => gettext("Exposure Compensation"),
        "AnalogueGainMode" => gettext("ISO Mode"),
        "AnalogueGain" => gettext("ISO"),
        "ExposureTimeMode" => gettext("Shutter Speed Mode"),
        "ExposureTime" => gettext("Shutter Speed"),
        "DigitalGain" => gettext("Digital Gain"),
        "AeExposureMode" => gettext("Exposure Program"),
        "AeConstraintMode" => gettext("Exposure Priority"),
        "AeMeteringMode" => gettext("Metering"),
        "AeFlickerMode" => gettext("Flicker Reduction"),
        "AeFlickerPeriod" => gettext("Flicker Period"),
        "FrameDurationLimits" => gettext("Frame Rate"),
        "AfMode" => gettext("Focus Mode"),
        "LensPosition" => gettext("Focus Distance"),
        "AfRange" => gettext("Focus Range"),
        "AfSpeed" => gettext("Focus Speed"),
        "AwbEnable" => gettext("Automatic White Balance"),
        "AwbMode" => gettext("Preset"),
        "ColourTemperature" => gettext("Colour Temperature"),
        "ColourGains" => gettext("Colour Gains"),
        "Brightness" => gettext("Brightness"),
        "Contrast" => gettext("Contrast"),
        "Saturation" => gettext("Saturation"),
        "Sharpness" => gettext("Sharpness"),
        "Gamma" => gettext("Gamma"),
        "NoiseReductionMode" => gettext("Noise Reduction"),
        "HdrMode" => gettext("HDR"),
        other => split_camel(other),
    };
    match KNOWN.iter().position(|(n, _)| *n == name) {
        Some(i) => (title, KNOWN[i].1, i),
        None => (title, Group::Other, KNOWN.len()),
    }
}

/// "AeFlickerMode" -> "Ae Flicker Mode", for controls nobody taught us about.
fn split_camel(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// "AnalogueGainModeAuto" -> "Auto": libcamera enumerators repeat their
/// control's name, or a shared word prefix, in every value.
fn enum_labels(control: &str, enums: &[(i32, String)]) -> Vec<String> {
    let names: Vec<&str> = enums.iter().map(|(_, n)| n.as_str()).collect();
    let first = names.first().copied().unwrap_or("");
    let mut len = names.iter().map(|n| first.bytes().zip(n.bytes()).take_while(|(a, b)| a == b).count()).min().unwrap_or(0);
    // Back off to a word boundary: every remainder must start a new word.
    let boundary = |len: usize| names.iter().all(|n| n[len..].starts_with(|c: char| c.is_ascii_uppercase()));
    while len > 0 && !boundary(len) {
        len = first[..len].rfind(|c: char| c.is_ascii_uppercase()).unwrap_or(0);
    }
    names
        .iter()
        .map(|n| {
            let rest = match n.strip_prefix(control) {
                Some(r) if r.starts_with(|c: char| c.is_ascii_uppercase()) => r,
                _ if names.len() > 1 && len < n.len() => &n[len..],
                _ => n,
            };
            split_camel(rest)
        })
        .collect()
}

pub fn format_value(name: &str, v: f64) -> String {
    match name {
        "ExposureTime" => {
            let s = v / 1e6;
            if s >= 0.5 { format!("{s:.1} s") } else { format!("1/{:.0} s", 1.0 / s.max(1e-6)) }
        }
        "AnalogueGain" => format!("{:.0}", v * 100.0),
        "DigitalGain" => format!("{v:.2}×"),
        "ExposureValue" => format!("{v:+.1} EV"),
        "ColourTemperature" => format!("{v:.0} K"),
        "LensPosition" if v <= 0.0 => "∞".into(),
        "LensPosition" if v < 1.0 => format!("{:.1} m", 1.0 / v),
        "LensPosition" => format!("{:.0} cm", 100.0 / v),
        _ if v.fract() == 0.0 => format!("{v:.0}"),
        _ => format!("{v:.2}"),
    }
}

/// Exposure spans four orders of magnitude, so its slider is logarithmic.
fn log_scale(name: &str) -> bool {
    name == "ExposureTime"
}

pub struct Panel {
    pub widget: gtk::Box,
    rows: HashMap<String, gtk::Widget>,
    values: Rc<RefCell<HashMap<String, Vec<f64>>>>,
    subtitles: HashMap<String, adw::ActionRow>,
    enums: HashMap<String, Vec<(i32, String)>>,
    /// Rows lent to the app's Capture group, taken back on drop.
    capture: (adw::PreferencesGroup, Vec<gtk::Widget>),
}

impl Drop for Panel {
    fn drop(&mut self) {
        for row in &self.capture.1 {
            self.capture.0.remove(row);
        }
    }
}

/// Which automatic modes are off, as the panel's current values say.
#[derive(Default, Clone, Copy)]
pub struct Manual {
    pub exposure: bool,
    pub gain: bool,
    pub white_balance: bool,
    pub focus: bool,
}

pub fn build(controls: &[ControlDesc], fps: Option<(f64, f64)>, capture: &adw::PreferencesGroup, on_change: OnChange) -> Rc<Panel> {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let groups = [
        (Group::Exposure, gettext("Exposure")),
        (Group::Focus, gettext("Focus")),
        (Group::WhiteBalance, gettext("White Balance")),
        (Group::Image, gettext("Image")),
        (Group::Other, gettext("Other")),
    ];
    let mut prefs: HashMap<Group, adw::PreferencesGroup> = HashMap::new();
    for (g, title) in &groups {
        let group = adw::PreferencesGroup::builder().title(title).visible(false).build();
        widget.append(&group);
        prefs.insert(*g, group);
    }

    let values: Rc<RefCell<HashMap<String, Vec<f64>>>> = Rc::default();
    let mut rows = HashMap::new();
    let mut subtitles = HashMap::new();
    let mut enums = HashMap::new();
    let mut resets: HashMap<Group, Vec<Reset>> = HashMap::new();
    let mut lent = Vec::new();
    // Set once every row exists, so a change can re-evaluate dependencies.
    let panel_cell: Rc<RefCell<Option<std::rc::Weak<Panel>>>> = Rc::default();

    let mut sorted: Vec<&ControlDesc> = controls.iter().filter(|c| !HIDDEN.contains(&c.name.as_str())).collect();
    sorted.sort_by_key(|c| (known(&c.name).2, c.name.clone()));
    for c in sorted {
        let (title, group, _) = known(&c.name);
        values.borrow_mut().insert(c.name.clone(), c.def.clone());
        let emit = {
            let values = values.clone();
            let on_change = on_change.clone();
            let panel_cell = panel_cell.clone();
            let (id, name) = (c.id, c.name.clone());
            move |v: Vec<f64>| {
                values.borrow_mut().insert(name.clone(), v.clone());
                on_change(id, v);
                if let Some(p) = panel_cell.borrow().as_ref().and_then(|w| w.upgrade()) {
                    p.update_dependencies();
                }
            }
        };

        let row: Option<(gtk::Widget, Reset)> = match (c.kind, c.name.as_str()) {
            (_, "FrameDurationLimits") => fps.map(|(lo, hi)| {
                let row = fps_row(&title, lo, hi, c, emit);
                let r = row.clone();
                (row.upcast(), Rc::new(move || r.set_selected(0)) as Reset)
            }),
            (Kind::Bool, _) if c.len == 1 => {
                let def = c.def.first().copied().unwrap_or(0.0) != 0.0;
                let row = adw::SwitchRow::builder().title(&title).active(def).build();
                row.connect_active_notify(move |r| emit(vec![r.is_active() as u8 as f64]));
                let r = row.clone();
                Some((row.upcast(), Rc::new(move || r.set_active(def)) as Reset))
            }
            (Kind::Int, _) if c.len == 1 && !c.enums.is_empty() => {
                let labels = enum_labels(&c.name, &c.enums);
                let names: Vec<&str> = labels.iter().map(String::as_str).collect();
                let model = gtk::StringList::new(&names);
                let def = c.def.first().copied().unwrap_or(0.0) as i32;
                let def = c.enums.iter().position(|(v, _)| *v == def).unwrap_or(0) as u32;
                let row = adw::ComboRow::builder().title(&title).model(&model).selected(def).build();
                let list = c.enums.clone();
                row.connect_selected_notify(move |r| {
                    if let Some((v, _)) = list.get(r.selected() as usize) {
                        emit(vec![*v as f64]);
                    }
                });
                enums.insert(c.name.clone(), c.enums.clone());
                let r = row.clone();
                Some((row.upcast(), Rc::new(move || r.set_selected(def)) as Reset))
            }
            (Kind::Int | Kind::Float, _) if c.max > c.min && (1..=4).contains(&c.len) => {
                let expander = (c.len > 1).then(|| adw::ExpanderRow::builder().title(&title).build());
                let current = Rc::new(RefCell::new(padded(&c.def, c.len, c.min)));
                let mut first = None;
                let mut row_resets = Vec::new();
                for i in 0..c.len {
                    let row_title = match (c.len, c.name.as_str(), i) {
                        (1, _, _) => title.clone(),
                        (2, "ColourGains", 0) => gettext("Red"),
                        (2, "ColourGains", 1) => gettext("Blue"),
                        _ => format!("{} {}", title, i + 1),
                    };
                    let (row, reset) = slider_row(&row_title, c, current.borrow()[i], {
                        let current = current.clone();
                        let emit = emit.clone();
                        move |v| {
                            current.borrow_mut()[i] = v;
                            emit(current.borrow().clone());
                        }
                    });
                    row_resets.push(reset);
                    if i == 0 {
                        subtitles.insert(c.name.clone(), row.clone());
                    }
                    match &expander {
                        Some(e) => e.add_row(&row),
                        None => first = Some(row),
                    }
                }
                let widget: Option<gtk::Widget> = expander.map(|e| e.upcast()).or(first.map(|r| r.upcast()));
                widget.map(|w| (w, Rc::new(move || row_resets.iter().for_each(|r| r())) as Reset))
            }
            _ => None,
        };

        if let Some((row, reset)) = row {
            if group == Group::Capture {
                capture.add(&row);
                lent.push(row.clone());
            } else {
                let g = &prefs[&group];
                g.add(&row);
                g.set_visible(true);
                resets.entry(group).or_default().push(reset);
            }
            rows.insert(c.name.clone(), row);
        }
    }

    for (group, list) in resets {
        let button = gtk::Button::builder()
            .icon_name("edit-undo-symbolic")
            .tooltip_text(gettext("Reset to Automatic"))
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&gettext("Reset to Automatic"))]);
        button.connect_clicked(move |_| list.iter().for_each(|r| r()));
        prefs[&group].set_header_suffix(Some(&button));
    }

    let panel = Rc::new(Panel { widget, rows, values, subtitles, enums, capture: (capture.clone(), lent) });
    panel_cell.replace(Some(Rc::downgrade(&panel)));
    panel.update_dependencies();
    panel
}

fn padded(def: &[f64], len: usize, fill: f64) -> Vec<f64> {
    (0..len).map(|i| def.get(i).copied().unwrap_or(fill)).collect()
}

fn slider_row(title: &str, c: &ControlDesc, value: f64, set: impl Fn(f64) + 'static) -> (adw::ActionRow, Reset) {
    let row = adw::ActionRow::builder().title(title).subtitle(format_value(&c.name, value)).build();
    let log = log_scale(&c.name) && c.min >= 0.0;
    let to_pos = move |v: f64| if log { (v.max(1.0)).ln() } else { v };
    let from_pos = move |p: f64| if log { p.exp() } else { p };
    let (lo, hi) = (to_pos(c.min.max(if log { 1.0 } else { c.min })), to_pos(c.max));
    let step = if c.kind == Kind::Int && !log { 1.0 } else { (hi - lo) / 200.0 };
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, lo, hi, step);
    scale.set_value(to_pos(value));
    scale.set_hexpand(true);
    scale.set_width_request(150);
    scale.set_valign(gtk::Align::Center);
    scale.update_property(&[gtk::accessible::Property::Label(title)]);
    let (name, int) = (c.name.clone(), c.kind == Kind::Int);
    let subtitle_row = row.clone();
    scale.connect_value_changed(move |s| {
        let mut v = from_pos(s.value());
        if int {
            v = v.round();
        }
        subtitle_row.set_subtitle(&format_value(&name, v));
        set(v);
    });
    row.add_suffix(&scale);
    let s = scale.clone();
    (row, Rc::new(move || s.set_value(to_pos(value))))
}

fn fps_row(title: &str, lo: f64, hi: f64, c: &ControlDesc, emit: impl Fn(Vec<f64>) + 'static) -> adw::ComboRow {
    let mut choices: Vec<f64> = [240.0, 120.0, 60.0, 30.0, 24.0, 15.0]
        .into_iter()
        .filter(|f| *f <= hi + 0.5 && *f >= lo - 0.5)
        .collect();
    if choices.is_empty() {
        choices.push(hi);
    }
    let mut labels = vec![gettext("Automatic")];
    labels.extend(choices.iter().map(|f| format!("{f:.0} fps")));
    let names: Vec<&str> = labels.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder().title(title).model(&gtk::StringList::new(&names)).build();
    let auto = c.def.clone();
    row.connect_selected_notify(move |r| {
        let v = match r.selected() {
            0 => auto.clone(),
            i => {
                let us = (1e6 / choices[i as usize - 1]).round();
                vec![us, us]
            }
        };
        emit(v);
    });
    row
}

impl Panel {
    pub fn value(&self, name: &str) -> Option<f64> {
        self.values.borrow().get(name).and_then(|v| v.first().copied())
    }

    /// Whether enum control `control` is set to the value named `…suffix`.
    pub fn is(&self, control: &str, suffix: &str) -> bool {
        let (Some(v), Some(list)) = (self.value(control), self.enums.get(control)) else { return false };
        list.iter().any(|(n, name)| *n as f64 == v && name.ends_with(suffix))
    }

    pub fn has(&self, name: &str) -> bool {
        self.rows.contains_key(name)
    }

    pub fn manual(&self) -> Manual {
        let ae_on = self.value("AeEnable").map(|v| v != 0.0);
        let manual = |mode: &str| self.value(mode).map(|v| v != 0.0);
        Manual {
            exposure: manual("ExposureTimeMode").unwrap_or(ae_on == Some(false)),
            gain: manual("AnalogueGainMode").unwrap_or(ae_on == Some(false)),
            white_balance: self.value("AwbEnable").map(|v| v == 0.0).unwrap_or(false),
            focus: self.value("AfMode").map(|v| v == 0.0).unwrap_or(false),
        }
    }

    /// Manual values only mean something while their automatic mode is off.
    pub fn update_dependencies(&self) {
        let m = self.manual();
        let ae_on = self.value("AeEnable").map(|v| v != 0.0).unwrap_or(true);
        let set = |name: &str, on: bool| {
            if let Some(w) = self.rows.get(name) {
                w.set_sensitive(on);
            }
        };
        set("ExposureTime", m.exposure);
        set("AnalogueGain", m.gain);
        set("ExposureValue", ae_on && !(m.exposure && m.gain));
        set("ColourTemperature", m.white_balance || !self.rows.contains_key("AwbEnable"));
        set("ColourGains", m.white_balance || !self.rows.contains_key("AwbEnable"));
        set("LensPosition", m.focus || !self.rows.contains_key("AfMode"));
    }

    /// While a value is automatic, its row shows what the camera chose.
    pub fn show_metadata(&self, meta: &Metadata) {
        for (name, row) in &self.subtitles {
            if row.is_sensitive() {
                continue;
            }
            if let Some(v) = meta.get(name) {
                row.set_subtitle(&format!("{} · {}", gettext("Auto"), format_value(name, v)));
            }
        }
    }

    /// Pick the enumerator of `control` whose name ends with `suffix`, as if
    /// the user had. False when the camera has no such value.
    pub fn select(&self, control: &str, suffix: &str) -> bool {
        let (Some(row), Some(list)) = (self.rows.get(control).and_then(|r| r.downcast_ref::<adw::ComboRow>()), self.enums.get(control)) else {
            return false;
        };
        match list.iter().position(|(_, n)| n.ends_with(suffix)) {
            Some(i) => {
                row.set_selected(i as u32);
                true
            }
            None => false,
        }
    }

    /// Move keyboard focus, and so the scrolled panel, to the first of
    /// `names` the camera has.
    pub fn focus(&self, names: &[&str]) {
        if let Some(row) = names.iter().find_map(|n| self.rows.get(*n)) {
            row.grab_focus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(split_camel("AeFlickerMode"), "Ae Flicker Mode");
        assert_eq!(format_value("ExposureTime", 4000.0), "1/250 s");
        assert_eq!(format_value("ExposureTime", 1_000_000.0), "1.0 s");
        assert_eq!(format_value("LensPosition", 0.0), "∞");
        assert_eq!(format_value("LensPosition", 0.5), "2.0 m");
        assert_eq!(format_value("LensPosition", 5.0), "20 cm");
        let e = |v: &[&str]| v.iter().enumerate().map(|(i, s)| (i as i32, s.to_string())).collect::<Vec<_>>();
        assert_eq!(enum_labels("AnalogueGainMode", &e(&["AnalogueGainModeAuto", "AnalogueGainModeManual"])), ["Auto", "Manual"]);
        assert_eq!(enum_labels("AeConstraintMode", &e(&["ConstraintNormal", "ConstraintHighlight", "ConstraintShadows"])), ["Normal", "Highlight", "Shadows"]);
        assert_eq!(enum_labels("AfMode", &e(&["AfModeManual", "AfModeAuto", "AfModeContinuous"])), ["Manual", "Auto", "Continuous"]);
        assert_eq!(enum_labels("HdrMode", &e(&["HdrModeOff", "HdrModeMultiExposureUnmerged"])), ["Off", "Multi Exposure Unmerged"]);
        assert_eq!(enum_labels("X", &e(&["AfModeManual", "AfModeMacro"])), ["Manual", "Macro"]);
    }
}
