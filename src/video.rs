// SPDX-License-Identifier: GPL-3.0-or-later
//! Video recording: capture frames are pushed into a GStreamer pipeline that
//! converts to NV12, encodes with the best encoder that actually works on
//! this machine, and muxes to MP4 with audio from the default source.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use gst::prelude::*;

#[derive(Debug, Clone)]
pub struct Encoder {
    pub element: &'static str,
    pub parser: &'static str,
    pub properties: &'static str,
}

/// Hardware first. An encoder that registers is not one that works -- a V4L2
/// encoder can fail at buffer allocation -- so each is probed with a short
/// real encode, once per process.
const CANDIDATES: &[Encoder] = &[
    Encoder { element: "v4l2h264enc", parser: "h264parse", properties: "" },
    Encoder { element: "vah264lpenc", parser: "h264parse", properties: "" },
    Encoder { element: "vah264enc", parser: "h264parse", properties: "" },
    Encoder { element: "v4l2h265enc", parser: "h265parse", properties: "" },
    Encoder { element: "vah265enc", parser: "h265parse", properties: "" },
    Encoder { element: "x264enc", parser: "h264parse", properties: "speed-preset=ultrafast tune=zerolatency bitrate=12000" },
    Encoder { element: "openh264enc", parser: "h264parse", properties: "bitrate=12000000" },
];

/// GStreamer is initialised on first use, off the startup path.
pub fn gst() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        gst::init().expect("GStreamer");
        perf!("gst-init");
    });
}

pub fn encoder() -> Option<&'static Encoder> {
    static PICK: OnceLock<Option<&'static Encoder>> = OnceLock::new();
    *PICK.get_or_init(|| {
        gst();
        let pick = CANDIDATES.iter().find(|e| {
            if gst::ElementFactory::find(e.element).is_none() {
                return false;
            }
            let ok = probe(e);
            log::info!("video encoder {}: {}", e.element, if ok { "works" } else { "fails" });
            ok
        });
        perf!("encoder-probed", "{}", pick.map_or("none", |e| e.element));
        pick
    })
}

fn probe(e: &Encoder) -> bool {
    let desc = format!(
        "videotestsrc num-buffers=3 ! video/x-raw,format=NV12,width=640,height=480,framerate=30/1 ! {} {} ! {} ! fakesink",
        e.element, e.properties, e.parser
    );
    let Ok(pipeline) = gst::parse::launch(&desc) else { return false };
    if pipeline.set_state(gst::State::Playing).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return false;
    }
    let ok = pipeline
        .bus()
        .and_then(|bus| bus.timed_pop_filtered(gst::ClockTime::from_seconds(5), &[gst::MessageType::Eos, gst::MessageType::Error]))
        .is_some_and(|m| matches!(m.view(), gst::MessageView::Eos(_)));
    let _ = pipeline.set_state(gst::State::Null);
    ok
}

/// The first audio source that can actually open a device; a session with no
/// sound server records silent video rather than failing.
fn audio_source() -> Option<&'static str> {
    ["pulsesrc", "pipewiresrc"].into_iter().find(|name| {
        let Ok(e) = gst::ElementFactory::make(name).build() else { return false };
        let ok = e.set_state(gst::State::Ready).is_ok();
        let _ = e.set_state(gst::State::Null);
        ok
    })
}

/// The frame size to encode: the largest standard 16:9 size the stream
/// covers, centre-cropped (4K from 4024x2268, 1080p from 2012x1132), else the
/// stream aligned down to 16 -- hardware encoders reject odd sizes, and the
/// soft ISP hands out sizes like 4024x2268.
fn encode_size(width: u32, height: u32) -> (u32, u32) {
    [(3840, 2160), (1920, 1080), (1280, 720)]
        .into_iter()
        .find(|&(w, h)| w <= width && h <= height && width * 100 / height.max(1) < 190)
        .unwrap_or((width & !15, height & !15))
}

pub fn videos_dir() -> PathBuf {
    let dir = relm4::gtk::glib::user_special_dir(relm4::gtk::glib::UserDirectory::Videos)
        .unwrap_or_else(|| relm4::gtk::glib::home_dir().join("Videos"))
        .join("Obscura");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub(crate) fn gst_format(fourcc: u32) -> Option<&'static str> {
    Some(match &fourcc.to_le_bytes() {
        b"XB24" => "RGBx",
        b"AB24" => "RGBA",
        b"XR24" => "BGRx",
        b"AR24" => "BGRA",
        b"NV12" => "NV12",
        b"YUYV" => "YUY2",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_sizes() {
        assert_eq!(super::encode_size(4024, 2268), (3840, 2160));
        assert_eq!(super::encode_size(2012, 1132), (1920, 1080));
        assert_eq!(super::encode_size(4024, 3032), (3840, 2160));
        assert_eq!(super::encode_size(640, 480), (640, 480));
        assert_eq!(super::encode_size(1000, 999), (992, 992));
    }
}

pub struct Recorder {
    pipeline: gst::Pipeline,
    src: gst_app::AppSrc,
    pub path: PathBuf,
    pub encoder: &'static str,
    width: u32,
    height: u32,
    format: gst_video::VideoFormat,
    pushed: AtomicU64,
    dropped: AtomicU64,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Recorder({})", self.path.display())
    }
}

impl Recorder {
    pub fn start(width: u32, height: u32, fourcc: u32, fps: f64, rotation: i32) -> Result<Self> {
        gst();
        let enc = encoder().context("no working video encoder")?;
        let format = gst_format(fourcc).context("viewfinder format cannot be recorded")?;
        let stem = crate::photo::file_stem().replacen("IMG_", "VID_", 1);
        let path = videos_dir().join(format!("{stem}.mp4"));

        let (ew, eh) = encode_size(width, height);
        let (cx, cy) = (width - ew, height - eh);
        let audio = audio_source()
            .map(|s| format!(" {s} ! queue ! audioconvert ! audioresample ! opusenc ! queue ! mux."))
            .unwrap_or_default();
        let desc = format!(
            "appsrc name=src is-live=true do-timestamp=true format=time max-buffers=3 leaky-type=downstream \
             ! queue max-size-buffers=3 leaky=downstream \
             ! videocrop left={} right={} top={} bottom={} ! videoconvert n-threads=4 \
             ! video/x-raw,format=NV12 ! {} {} ! {} ! queue ! mp4mux name=mux \
             ! filesink location=\"{}\"{audio}",
            cx / 2,
            cx - cx / 2,
            cy / 2,
            cy - cy / 2,
            enc.element,
            enc.properties,
            enc.parser,
            path.display()
        );
        let pipeline = gst::parse::launch(&desc)?.downcast::<gst::Pipeline>().map_err(|_| anyhow!("not a pipeline"))?;
        let src = pipeline.by_name("src").unwrap().downcast::<gst_app::AppSrc>().unwrap();
        let fps = gst::Fraction::approximate_f64(fps.clamp(1.0, 240.0)).unwrap_or(gst::Fraction::new(30, 1));
        src.set_caps(Some(
            &gst::Caps::builder("video/x-raw")
                .field("format", format)
                .field("width", width as i32)
                .field("height", height as i32)
                .field("framerate", fps)
                .build(),
        ));

        // Rotation goes in the container, like phone cameras do it.
        let mux = pipeline.by_name("mux").unwrap();
        if let Ok(setter) = mux.dynamic_cast::<gst::TagSetter>() {
            let orientation = match rotation.rem_euclid(360) {
                90 => "rotate-90",
                180 => "rotate-180",
                270 => "rotate-270",
                _ => "rotate-0",
            };
            let mut tags = gst::TagList::new();
            tags.get_mut().unwrap().add::<gst::tags::ImageOrientation>(&orientation, gst::TagMergeMode::Replace);
            setter.merge_tags(&tags, gst::TagMergeMode::Replace);
        }

        pipeline.set_state(gst::State::Playing)?;
        Ok(Self {
            pipeline,
            src,
            path,
            encoder: enc.element,
            width,
            height,
            format: gst_video::VideoFormat::from_string(format),
            pushed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        })
    }

    /// Copy one frame in. Called from the capture thread.
    pub fn push(&self, data: &[u8], stride: u32) {
        let mut buffer = gst::Buffer::from_mut_slice(data.to_vec());
        {
            let b = buffer.get_mut().unwrap();
            let strides: &[i32] = if self.format == gst_video::VideoFormat::Nv12 {
                &[stride as i32, stride as i32]
            } else {
                &[stride as i32]
            };
            let offsets: &[usize] = if self.format == gst_video::VideoFormat::Nv12 {
                &[0, (stride * self.height) as usize]
            } else {
                &[0]
            };
            let _ = gst_video::VideoMeta::add_full(b, gst_video::VideoFrameFlags::empty(), self.format, self.width, self.height, offsets, strides);
        }
        match self.src.push_buffer(buffer) {
            Ok(_) => self.pushed.fetch_add(1, Ordering::Relaxed),
            Err(_) => self.dropped.fetch_add(1, Ordering::Relaxed),
        };
    }

    /// Finish the file. Blocks until the muxer has written the index.
    pub fn stop(&self) -> Result<PathBuf> {
        let _ = self.src.end_of_stream();
        // Sources without an end (the microphone) need EOS on the pipeline.
        self.pipeline.send_event(gst::event::Eos::new());
        let bus = self.pipeline.bus().unwrap();
        let msg = bus.timed_pop_filtered(gst::ClockTime::from_seconds(15), &[gst::MessageType::Eos, gst::MessageType::Error]);
        let _ = self.pipeline.set_state(gst::State::Null);
        log::info!(
            "recorded {} with {}: {} frames pushed, {} dropped at the source",
            self.path.display(),
            self.encoder,
            self.pushed.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed)
        );
        match msg.as_ref().map(|m| m.view()) {
            Some(gst::MessageView::Error(e)) => Err(anyhow!("{}", e.error())),
            Some(_) => Ok(self.path.clone()),
            None => Err(anyhow!("recording did not finish")),
        }
    }
}
