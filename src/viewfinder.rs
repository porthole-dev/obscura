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
            let (w, h) = (self.obj().width() as f32, self.obj().height() as f32);
            let rotation = self.rotation.get().rem_euclid(360);
            let (tw, th) = (texture.width() as f32, texture.height() as f32);
            // Size of the upright image, then scaled to fit ("contain").
            let (uw, uh) = if rotation % 180 == 0 { (tw, th) } else { (th, tw) };
            let scale = if self.cover.get() { (w / uw).max(h / uh) } else { (w / uw).min(h / uh) };
            snapshot.save();
            snapshot.translate(&graphene::Point::new(w / 2.0, h / 2.0));
            // Mirror the upright picture, not the sensor image: after a
            // quarter turn a sensor-space mirror is an upside-down flip.
            if self.mirror.get() {
                snapshot.scale(-1.0, 1.0);
            }
            snapshot.rotate(rotation as f32);
            let (dw, dh) = (tw * scale, th * scale);
            snapshot.append_scaled_texture(
                &texture,
                gtk::gsk::ScalingFilter::Linear,
                &graphene::Rect::new(-dw / 2.0, -dh / 2.0, dw, dh),
            );
            snapshot.restore();
        }
    }
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

    /// Where a point on the widget lands on the sensor image, normalised to
    /// 0..1 in sensor coordinates, or None outside the image.
    pub fn to_sensor(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let texture = self.imp().texture.borrow().clone()?;
        let rotation = self.imp().rotation.get().rem_euclid(360);
        let (w, h) = (self.width() as f64, self.height() as f64);
        let (tw, th) = (texture.width() as f64, texture.height() as f64);
        let (uw, uh) = if rotation % 180 == 0 { (tw, th) } else { (th, tw) };
        let scale = (w / uw).min(h / uh);
        // Centered, in upright image pixels.
        let (mut u, v) = ((x - w / 2.0) / scale, (y - h / 2.0) / scale);
        if self.imp().mirror.get() {
            u = -u;
        }
        // Undo the clockwise rotation.
        let (sx, sy) = match rotation {
            90 => (v, -u),
            180 => (-u, -v),
            270 => (-v, u),
            _ => (u, v),
        };
        let (nx, ny) = (sx / tw + 0.5, sy / th + 0.5);
        ((0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny)).then_some((nx, ny))
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
