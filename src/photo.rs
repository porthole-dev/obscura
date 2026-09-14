// SPDX-License-Identifier: GPL-3.0-or-later
//! Writing stills: JPEG with EXIF through GStreamer (jpegenc + jifmux turn
//! GstTags into EXIF), and DNG through [`crate::dng`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use gst::prelude::*;

use crate::camera::Still;

pub fn pictures_dir() -> PathBuf {
    let dir = relm4::gtk::glib::user_special_dir(relm4::gtk::glib::UserDirectory::Pictures)
        .unwrap_or_else(|| relm4::gtk::glib::home_dir().join("Pictures"))
        .join("Obscura");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn file_stem() -> String {
    let now = relm4::gtk::glib::DateTime::now_local().expect("local time");
    format!("IMG_{}", now.format("%Y%m%d_%H%M%S").map(|s| s.to_string()).unwrap_or_default())
}

fn gst_format(fourcc: u32) -> Option<&'static str> {
    Some(match &fourcc.to_le_bytes() {
        b"XB24" => "RGBx",
        b"AB24" => "RGBA",
        b"XR24" => "BGRx",
        b"AR24" => "BGRA",
        _ => return None,
    })
}

/// The videoflip method that turns a buffer `rotation` degrees clockwise.
fn flip_method(rotation: i32) -> &'static str {
    match rotation.rem_euclid(360) {
        90 => "clockwise",
        180 => "rotate-180",
        270 => "counterclockwise",
        _ => "none",
    }
}

#[link(name = "gsttag-1.0")]
unsafe extern "C" {
    // Registers the EXIF-backed capture tags (capturing-shutter-speed,
    // capturing-iso-speed, ...); nothing else in-process does it first.
    fn gst_tag_register_musicbrainz_tags();
}

pub fn save_jpeg(still: &Still, path: &Path) -> Result<()> {
    // SAFETY: idempotent (GOnce inside) and thread-safe.
    unsafe { gst_tag_register_musicbrainz_tags() };
    let format = gst_format(still.fourcc).context("viewfinder format has no JPEG path")?;
    let (w, h, stride) = (still.width as usize, still.height as usize, still.stride as usize);
    // Tight rows: the capture buffer may be padded past width * 4.
    let mut pixels = Vec::with_capacity(w * h * 4);
    for row in still.rgba.chunks(stride).take(h) {
        pixels.extend_from_slice(&row[..w * 4]);
    }
    if pixels.len() != w * h * 4 {
        bail!("short frame: {} of {} bytes", pixels.len(), w * h * 4);
    }

    let pipeline = gst::parse::launch(&format!(
        "appsrc name=src ! videoconvert ! videoflip method={} ! jpegenc quality=92 ! jifmux name=mux ! filesink location=\"{}\"",
        flip_method(still.info.rotation),
        path.display()
    ))?
    .downcast::<gst::Pipeline>()
    .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
    let src = pipeline.by_name("src").unwrap().downcast::<gst_app::AppSrc>().unwrap();
    src.set_caps(Some(
        &gst::Caps::builder("video/x-raw")
            .field("format", format)
            .field("width", w as i32)
            .field("height", h as i32)
            .field("framerate", gst::Fraction::new(0, 1))
            .build(),
    ));
    src.set_format(gst::Format::Time);

    let mux = pipeline.by_name("mux").unwrap();
    let setter = mux.dynamic_cast::<gst::TagSetter>().unwrap();
    let mut tags = gst::TagList::new();
    {
        let t = tags.get_mut().unwrap();
        t.add::<gst::tags::DeviceModel>(&still.info.model.as_str(), gst::TagMergeMode::Replace);
        t.add::<gst::tags::ApplicationName>(&"Obscura", gst::TagMergeMode::Replace);
        // The pixels are turned upright above, so every viewer agrees.
        t.add::<gst::tags::ImageOrientation>(&"rotate-0", gst::TagMergeMode::Replace);
        let now = gst::DateTime::from_g_date_time(relm4::gtk::glib::DateTime::now_local()?);
        t.add::<gst::tags::DateTime>(&now, gst::TagMergeMode::Replace);
        if let Some(us) = still.metadata.get("ExposureTime") {
            // shutter speed as a fraction of a second
            let _ = t.add_generic("capturing-shutter-speed", gst::Fraction::new(us.round() as i32, 1_000_000), gst::TagMergeMode::Replace);
        }
        if let Some(gain) = still.metadata.get("AnalogueGain") {
            let digital = still.metadata.get("DigitalGain").unwrap_or(1.0);
            let _ = t.add_generic("capturing-iso-speed", (gain * digital * 100.0).round() as i32, gst::TagMergeMode::Replace);
        }
    }
    setter.merge_tags(&tags, gst::TagMergeMode::Replace);

    pipeline.set_state(gst::State::Playing)?;
    src.push_buffer(gst::Buffer::from_mut_slice(pixels))?;
    src.end_of_stream()?;
    let bus = pipeline.bus().unwrap();
    let result = match bus.timed_pop_filtered(gst::ClockTime::from_seconds(20), &[gst::MessageType::Eos, gst::MessageType::Error]) {
        Some(msg) => match msg.view() {
            gst::MessageView::Error(e) => Err(anyhow::anyhow!("{}", e.error())),
            _ => Ok(()),
        },
        None => Err(anyhow::anyhow!("JPEG encoding timed out")),
    };
    pipeline.set_state(gst::State::Null)?;
    result
}

/// Save a still as JPEG, plus DNG when it carries raw data and `raw` is set.
/// Returns the JPEG path.
pub fn save(still: &Still, raw: bool) -> Result<PathBuf> {
    let dir = pictures_dir();
    let stem = file_stem();
    let jpeg = dir.join(format!("{stem}.jpg"));
    save_jpeg(still, &jpeg)?;
    if raw && let Some(image) = &still.raw {
        crate::dng::write(image, still, &dir.join(format!("{stem}.dng")))?;
    }
    Ok(jpeg)
}
