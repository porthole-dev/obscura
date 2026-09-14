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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Group {
    Exposure,
    Colour,
    Focus,
    Image,
    Other,
}

struct Known {
    title: String,
    group: Group,
}

fn known(name: &str) -> Known {
    let (title, group) = match name {
        "AeEnable" => (gettext("Automatic Exposure"), Group::Exposure),
        "ExposureTimeMode" => (gettext("Shutter Speed Mode"), Group::Exposure),
        "ExposureTime" => (gettext("Shutter Speed"), Group::Exposure),
        "AnalogueGainMode" => (gettext("Sensitivity Mode"), Group::Exposure),
        "AnalogueGain" => (gettext("Sensitivity"), Group::Exposure),
        "DigitalGain" => (gettext("Digital Gain"), Group::Exposure),
        "ExposureValue" => (gettext("Exposure Compensation"), Group::Exposure),
        "AeMeteringMode" => (gettext("Metering"), Group::Exposure),
        "AeConstraintMode" => (gettext("Exposure Constraint"), Group::Exposure),
        "AeExposureMode" => (gettext("Exposure Program"), Group::Exposure),
        "AeFlickerMode" => (gettext("Flicker Reduction"), Group::Exposure),
        "AeFlickerPeriod" => (gettext("Flicker Period"), Group::Exposure),
        "FrameDurationLimits" => (gettext("Frame Rate"), Group::Exposure),
        "AwbEnable" => (gettext("Automatic White Balance"), Group::Colour),
        "AwbMode" => (gettext("White Balance Preset"), Group::Colour),
        "ColourTemperature" => (gettext("Colour Temperature"), Group::Colour),
        "ColourGains" => (gettext("Colour Gains"), Group::Colour),
        "Saturation" => (gettext("Saturation"), Group::Colour),
        "AfMode" => (gettext("Focus Mode"), Group::Focus),
        "AfRange" => (gettext("Focus Range"), Group::Focus),
        "AfSpeed" => (gettext("Focus Speed"), Group::Focus),
        "AfMetering" => (gettext("Focus Area"), Group::Focus),
        "AfTrigger" => (gettext("Focus Trigger"), Group::Focus),
        "AfPause" => (gettext("Pause Focus"), Group::Focus),
        "LensPosition" => (gettext("Focus Distance"), Group::Focus),
        "Brightness" => (gettext("Brightness"), Group::Image),
        "Contrast" => (gettext("Contrast"), Group::Image),
        "Gamma" => (gettext("Gamma"), Group::Image),
        "Sharpness" => (gettext("Sharpness"), Group::Image),
        "NoiseReductionMode" => (gettext("Noise Reduction"), Group::Image),
        "HdrMode" => (gettext("HDR"), Group::Image),
        other => (split_camel(other), Group::Other),
    };
    Known { title, group }
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

fn format_value(name: &str, v: f64) -> String {
    match name {
        "ExposureTime" | "ExposureTimeMetadata" => {
            let s = v / 1e6;
            if s >= 0.5 { format!("{s:.1} s") } else { format!("1/{:.0} s", 1.0 / s.max(1e-6)) }
        }
        "AnalogueGain" => format!("ISO {:.0}  ({v:.2}×)", v * 100.0),
        "DigitalGain" => format!("{v:.2}×"),
        "ExposureValue" => format!("{v:+.1} EV"),
        "ColourTemperature" => format!("{v:.0} K"),
        "LensPosition" if v <= 0.0 => "∞".into(),
        "LensPosition" => format!("{:.2} m  ({v:.1} dpt)", 1.0 / v),
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
}

pub fn build(controls: &[ControlDesc], fps: Option<(f64, f64)>, on_change: OnChange) -> Rc<Panel> {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 18);
    let groups = [
        (Group::Exposure, gettext("Exposure")),
        (Group::Focus, gettext("Focus")),
        (Group::Colour, gettext("Colour")),
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
    // Set once every row exists, so a change can re-evaluate dependencies.
    let panel_cell: Rc<RefCell<Option<std::rc::Weak<Panel>>>> = Rc::default();

    for c in controls {
        let k = known(&c.name);
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

        let row: Option<gtk::Widget> = match (c.kind, c.name.as_str()) {
            (_, "FrameDurationLimits") => fps.map(|(lo, hi)| fps_row(&k.title, lo, hi, c, emit).upcast()),
            (Kind::Bool, _) if c.len == 1 => {
                let row = adw::SwitchRow::builder()
                    .title(&k.title)
                    .active(c.def.first().copied().unwrap_or(0.0) != 0.0)
                    .build();
                row.connect_active_notify(move |r| emit(vec![r.is_active() as u8 as f64]));
                Some(row.upcast())
            }
            (Kind::Int, _) if c.len == 1 && !c.enums.is_empty() => {
                let names: Vec<&str> = c.enums.iter().map(|(_, n)| n.as_str()).collect();
                let model = gtk::StringList::new(&names);
                let def = c.def.first().copied().unwrap_or(0.0) as i32;
                let row = adw::ComboRow::builder()
                    .title(&k.title)
                    .model(&model)
                    .selected(c.enums.iter().position(|(v, _)| *v == def).unwrap_or(0) as u32)
                    .build();
                let enums = c.enums.clone();
                row.connect_selected_notify(move |r| {
                    if let Some((v, _)) = enums.get(r.selected() as usize) {
                        emit(vec![*v as f64]);
                    }
                });
                Some(row.upcast())
            }
            (Kind::Int | Kind::Float, _) if c.max > c.min && (1..=4).contains(&c.len) => {
                let expander = (c.len > 1).then(|| adw::ExpanderRow::builder().title(&k.title).build());
                let current = Rc::new(RefCell::new(padded(&c.def, c.len, c.min)));
                let mut first = None;
                for i in 0..c.len {
                    let title = match (c.len, c.name.as_str(), i) {
                        (1, _, _) => k.title.clone(),
                        (2, "ColourGains", 0) => gettext("Red"),
                        (2, "ColourGains", 1) => gettext("Blue"),
                        _ => format!("{} {}", k.title, i + 1),
                    };
                    let row = slider_row(&title, c, current.borrow()[i], {
                        let current = current.clone();
                        let emit = emit.clone();
                        move |v| {
                            current.borrow_mut()[i] = v;
                            emit(current.borrow().clone());
                        }
                    });
                    if i == 0 {
                        subtitles.insert(c.name.clone(), row.clone());
                    }
                    match &expander {
                        Some(e) => e.add_row(&row),
                        None => first = Some(row),
                    }
                }
                expander.map(|e| e.upcast()).or(first.map(|r| r.upcast()))
            }
            _ => None,
        };

        if let Some(row) = row {
            let group = &prefs[&k.group];
            group.add(&row);
            group.set_visible(true);
            rows.insert(c.name.clone(), row);
        }
    }

    let panel = Rc::new(Panel { widget, rows, values, subtitles });
    panel_cell.replace(Some(Rc::downgrade(&panel)));
    panel.update_dependencies();
    panel
}

fn padded(def: &[f64], len: usize, fill: f64) -> Vec<f64> {
    (0..len).map(|i| def.get(i).copied().unwrap_or(fill)).collect()
}

fn slider_row(title: &str, c: &ControlDesc, value: f64, set: impl Fn(f64) + 'static) -> adw::ActionRow {
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
    row
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
    fn value(&self, name: &str) -> Option<f64> {
        self.values.borrow().get(name).and_then(|v| v.first().copied())
    }

    /// Manual values only mean something while their automatic mode is off.
    pub fn update_dependencies(&self) {
        let ae_on = self.value("AeEnable").map(|v| v != 0.0);
        let manual = |mode: &str| self.value(mode).map(|v| v != 0.0);
        let exposure_manual = manual("ExposureTimeMode").unwrap_or(ae_on == Some(false));
        let gain_manual = manual("AnalogueGainMode").unwrap_or(ae_on == Some(false));
        let awb_off = self.value("AwbEnable").map(|v| v == 0.0).unwrap_or(true);
        let af_manual = self.value("AfMode").map(|v| v == 0.0).unwrap_or(true);
        let set = |name: &str, on: bool| {
            if let Some(w) = self.rows.get(name) {
                w.set_sensitive(on);
            }
        };
        set("ExposureTime", exposure_manual);
        set("AnalogueGain", gain_manual);
        set("ExposureValue", ae_on.unwrap_or(true) && !(exposure_manual && gain_manual));
        set("ColourTemperature", awb_off);
        set("ColourGains", awb_off);
        set("LensPosition", af_manual);
    }

    /// While a value is automatic, its row shows what the camera chose.
    pub fn show_metadata(&self, meta: &Metadata) {
        for (name, row) in &self.subtitles {
            if row.is_sensitive() {
                continue;
            }
            if let Some(v) = meta.get(name) {
                row.set_subtitle(&format_value(name, v));
            }
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
    }
}
