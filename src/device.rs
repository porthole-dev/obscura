// SPDX-License-Identifier: GPL-3.0-or-later
//! What the device around the camera can tell and do: which way up it is
//! (iio-sensor-proxy) and a shutter sound or buzz (feedbackd). Both are
//! optional D-Bus services; without them nothing happens.

use std::cell::RefCell;
use std::rc::Rc;

use relm4::gtk::{gio, glib, prelude::*};

use crate::APP_ID;

/// Play a feedbackd event, such as "camera-shutter", with the user's
/// feedback theme: sound, vibration or LED as the device and profile allow.
pub fn feedback(event: &str) {
    let Some(bus) = relm4::main_application().dbus_connection() else { return };
    let hints = glib::VariantDict::new(None).end();
    bus.call(
        Some("org.sigxcpu.Feedback"),
        "/org/sigxcpu/Feedback",
        "org.sigxcpu.Feedback",
        "TriggerFeedback",
        Some(&(APP_ID, event, hints, -1i32).to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        -1,
        None::<&gio::Cancellable>,
        |_| {},
    );
}

/// Degrees the device is turned clockwise from its natural orientation, as
/// iio-sensor-proxy names it; None for "undefined" (lying flat).
fn degrees(orientation: &str) -> Option<i32> {
    Some(match orientation {
        "normal" => 0,
        "left-up" => 90,
        "bottom-up" => 180,
        "right-up" => 270,
        _ => return None,
    })
}

/// The accelerometer, claimed while the camera runs.
#[derive(Clone, Default)]
pub struct Orientation {
    proxy: Rc<RefCell<Option<gio::DBusProxy>>>,
}

impl Orientation {
    /// Report every change of the device's orientation to `changed`.
    pub fn watch(changed: impl Fn(i32) + 'static) -> Self {
        let this = Self::default();
        let slot = this.proxy.clone();
        gio::DBusProxy::for_bus(
            gio::BusType::System,
            gio::DBusProxyFlags::NONE,
            None,
            "net.hadess.SensorProxy",
            "/net/hadess/SensorProxy",
            "net.hadess.SensorProxy",
            None::<&gio::Cancellable>,
            move |result| {
                let Ok(proxy) = result else { return };
                if proxy.name_owner().is_none() {
                    return;
                }
                let read = move |p: &gio::DBusProxy| p.cached_property("AccelerometerOrientation").and_then(|v| v.get::<String>()).as_deref().and_then(degrees);
                proxy.connect_local("g-properties-changed", false, move |args| {
                    if let Some(d) = args.first().and_then(|a| a.get::<gio::DBusProxy>().ok()).and_then(|p| read(&p)) {
                        changed(d);
                    }
                    None
                });
                slot.replace(Some(proxy));
                Orientation { proxy: slot }.claim(true);
            },
        );
        this
    }

    /// Keep the sensor on only while it is needed.
    pub fn claim(&self, on: bool) {
        if let Some(p) = self.proxy.borrow().as_ref() {
            let method = if on { "ClaimAccelerometer" } else { "ReleaseAccelerometer" };
            p.call(method, None, gio::DBusCallFlags::NONE, -1, None::<&gio::Cancellable>, |_| {});
        }
    }
}

/// Degrees to turn a camera's buffers clockwise so the picture is upright for
/// someone holding the device `device` degrees clockwise from natural. A
/// front camera sees that turn mirrored.
pub fn upright(sensor: i32, front: bool, device: i32) -> i32 {
    (if front { sensor - device } else { sensor + device }).rem_euclid(360)
}

#[cfg(test)]
mod tests {
    #[test]
    fn landscape_turns_the_picture_back() {
        // Pixel 2 XL back camera (270) held with its left edge up: the
        // world looks a quarter turn anticlockwise, so add a clockwise one.
        assert_eq!(super::upright(270, false, 90), 0);
        assert_eq!(super::upright(90, true, 90), 0);
        assert_eq!(super::upright(270, false, 0), 270);
        assert_eq!(super::degrees("right-up"), Some(270));
    }
}
