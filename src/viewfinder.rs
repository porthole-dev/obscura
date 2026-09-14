// SPDX-License-Identifier: GPL-3.0-or-later
//! The viewfinder: camera frames become GdkTextures, imported straight from
//! the capture dmabufs when the display can, and drawn rotated upright.

use std::cell::{Cell, RefCell};

use relm4::gtk::{self, gdk, glib, graphene, prelude::*, subclass::prelude::*};

use crate::camera::Frame;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Viewfinder {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub rotation: Cell<i32>,
        pub mirror: Cell<bool>,
        /// Fill the widget and crop, instead of fitting inside it.
        pub cover: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Viewfinder {
        const NAME: &'static str = "ObscuraViewfinder";
        type Type = super::Viewfinder;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("viewfinder");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for Viewfinder {}

    impl WidgetImpl for Viewfinder {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(texture) = self.texture.borrow().clone() else { return };
            let Some(l) = self.obj().layout() else { return };
            let (tw, th) = (texture.width() as f32 * l.scale, texture.height() as f32 * l.scale);
            snapshot.save();
            snapshot.translate(&graphene::Point::new(l.x + l.width / 2.0, l.y + l.height / 2.0));
            // Mirror the upright picture, not the sensor image: after a
            // quarter turn a sensor-space mirror is an upside-down flip.
            if self.mirror.get() {
                snapshot.scale(-1.0, 1.0);
            }
            snapshot.rotate(self.rotation.get().rem_euclid(360) as f32);
            snapshot.append_scaled_texture(
                &texture,
                gtk::gsk::ScalingFilter::Linear,
                &graphene::Rect::new(-tw / 2.0, -th / 2.0, tw, th),
            );
            snapshot.restore();
        }
    }
}

/// Where the upright picture lands in the widget.
pub struct Layout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Widget pixels per image pixel.
    pub scale: f32,
}

glib::wrapper! {
    pub struct Viewfinder(ObjectSubclass<imp::Viewfinder>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Viewfinder {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Viewfinder {
    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        self.imp().texture.replace(texture);
        self.queue_draw();
    }

    pub fn set_cover(&self, cover: bool) {
        self.imp().cover.set(cover);
        self.queue_draw();
    }

    pub fn set_rotation(&self, degrees: i32, mirror: bool) {
        self.imp().rotation.set(degrees);
        self.imp().mirror.set(mirror);
        self.queue_draw();
    }

    pub fn layout(&self) -> Option<Layout> {
        let texture = self.imp().texture.borrow().clone()?;
        let size = (self.width() as f32, self.height() as f32);
        Some(fit(size, (texture.width() as f32, texture.height() as f32), self.imp().rotation.get(), self.imp().cover.get()))
    }

    /// Where a point on the widget lands on the sensor image, normalised to
    /// 0..1 in sensor coordinates, or None outside the image.
    pub fn to_sensor(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        to_sensor(&self.layout()?, self.imp().rotation.get(), self.imp().mirror.get(), x, y)
    }
}

/// Rule-of-thirds lines over wherever `viewfinder` draws the picture. A
/// separate widget, so the viewfinder stays a lone texture the compositor
/// can take as an overlay.
pub fn grid(viewfinder: &Viewfinder) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder().can_target(false).build();
    let vf = viewfinder.downgrade();
    area.set_draw_func(move |_, cr, _, _| {
        let Some(l) = vf.upgrade().and_then(|v| v.layout()) else { return };
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
        cr.set_line_width(1.0);
        for i in 1..3 {
            let f = i as f64 / 3.0;
            let x = (l.x as f64 + l.width as f64 * f).round() + 0.5;
            let y = (l.y as f64 + l.height as f64 * f).round() + 0.5;
            cr.move_to(x, l.y as f64);
            cr.line_to(x, (l.y + l.height) as f64);
            cr.move_to(l.x as f64, y);
            cr.line_to((l.x + l.width) as f64, y);
        }
        let _ = cr.stroke();
    });
    area
}

/// Fitted (or, with cover, filled) and centred; with room to spare
/// vertically the picture sits at the top, like phone cameras, leaving the
/// bottom to the capture controls.
fn fit((w, h): (f32, f32), (tw, th): (f32, f32), rotation: i32, cover: bool) -> Layout {
    let (uw, uh) = if rotation.rem_euclid(180) == 0 { (tw, th) } else { (th, tw) };
    let scale = if cover { (w / uw).max(h / uh) } else { (w / uw).min(h / uh) };
    let (width, height) = (uw * scale, uh * scale);
    let y = if height < h { 0.0 } else { (h - height) / 2.0 };
    Layout { x: (w - width) / 2.0, y, width, height, scale }
}

fn to_sensor(l: &Layout, rotation: i32, mirror: bool, x: f64, y: f64) -> Option<(f64, f64)> {
    let rotation = rotation.rem_euclid(360);
    // Centred, in upright image pixels.
    let mut u = (x - (l.x + l.width / 2.0) as f64) / l.scale as f64;
    let v = (y - (l.y + l.height / 2.0) as f64) / l.scale as f64;
    if mirror {
        u = -u;
    }
    // Undo the clockwise rotation.
    let (sx, sy) = match rotation {
        90 => (v, -u),
        180 => (-u, -v),
        270 => (-v, u),
        _ => (u, v),
    };
    let (tw, th) = if rotation % 180 == 0 { (l.width, l.height) } else { (l.height, l.width) };
    let (nx, ny) = (sx / (tw / l.scale) as f64 + 0.5, sy / (th / l.scale) as f64 + 0.5);
    ((0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny)).then_some((nx, ny))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_land_where_the_picture_is_drawn() {
        // A 4:3 sensor turned 270 degrees in a portrait phone window: the
        // picture is 480x640 at the top.
        let l = fit((480.0, 900.0), (4032.0, 3024.0), 270, false);
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(near(l.x, 0.0) && near(l.y, 0.0) && near(l.width, 480.0) && near(l.height, 640.0));
        // The buffer's right edge is drawn at the top.
        let (nx, ny) = to_sensor(&l, 270, false, 240.0, 1.0).unwrap();
        assert!((nx - 1.0).abs() < 0.01 && (ny - 0.5).abs() < 0.01, "{nx} {ny}");
        // Mirrored (selfie) preview: the left of the screen is the right of the upright picture.
        let (nx, ny) = to_sensor(&l, 90, true, 1.0, 320.0).unwrap();
        assert!((nx - 0.5).abs() < 0.01 && (ny - 0.0).abs() < 0.01, "{nx} {ny}");
        assert_eq!(to_sensor(&l, 270, false, 240.0, 700.0), None);
    }
}

/// Turn a frame into a texture. The dmabuf import keeps the frame (and so its
/// capture request) alive until GTK releases the texture. Err means the
/// import is not possible and the caller should switch to copied frames.
pub fn texture(frame: Frame) -> Result<gdk::Texture, ()> {
    if let Some(bytes) = frame.bytes.as_ref() {
        let format = memory_format(frame.fourcc).ok_or(())?;
        let bytes = glib::Bytes::from(bytes.as_slice());
        return Ok(gdk::MemoryTexture::new(
            frame.width as i32,
            frame.height as i32,
            format,
            &bytes,
            frame.stride as usize,
        )
        .upcast());
    }

    let display = gdk::Display::default().ok_or(())?;
    let nv12 = frame.fourcc == u32::from_le_bytes(*b"NV12");
    let mut builder = gdk::DmabufTextureBuilder::new()
        .set_display(&display)
        .set_width(frame.width)
        .set_height(frame.height)
        .set_fourcc(frame.fourcc)
        .set_modifier(0)
        .set_n_planes(if nv12 { 2 } else { 1 })
        .set_stride(0, frame.stride)
        .set_offset(0, frame.offset);
    // SAFETY: the fd outlives the texture; see below.
    builder = unsafe { builder.set_fd(0, frame.fd) };
    if nv12 {
        builder = unsafe { builder.set_fd(1, frame.fd) }
            .set_stride(1, frame.stride)
            .set_offset(1, frame.offset + frame.stride * frame.height);
    }
    // SAFETY: the fd belongs to a capture buffer that is not requeued until
    // `frame` is dropped, which is exactly when GTK calls the release func.
    unsafe { builder.build_with_release_func(move || drop(frame)) }.map_err(|e| {
        log::warn!("dmabuf import failed, falling back to copies: {e}");
    })
}

fn memory_format(fourcc: u32) -> Option<gdk::MemoryFormat> {
    Some(match &fourcc.to_le_bytes() {
        b"XB24" => gdk::MemoryFormat::R8g8b8x8,
        b"AB24" => gdk::MemoryFormat::R8g8b8a8,
        b"XR24" => gdk::MemoryFormat::B8g8r8x8,
        b"AR24" => gdk::MemoryFormat::B8g8r8a8,
        _ => return None,
    })
}
