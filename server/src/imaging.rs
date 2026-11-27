//! Image checks with the `image` crate: formats, dimensions, and the still first frame
//! of an animated avatar. Decoding is CPU and memory heavy, so it runs one at a time
//! (`AppState::imaging`) on a blocking thread, within `image::Limits`.

use crate::{
    error::{internal, Result},
    state::AppState,
};
use image::{
    codecs::{gif::GifDecoder, webp::WebPDecoder},
    imageops::FilterType,
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage,
};
use proto::limits;
use std::io::Cursor;

/// What the header of an image says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Info {
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
}

impl Info {
    pub fn mime(&self) -> &'static str {
        match self.format {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::WebP => "image/webp",
            ImageFormat::Gif => "image/gif",
            _ => "application/octet-stream",
        }
    }
}

fn decode_limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(limits::IMAGE_SIDE);
    l.max_image_height = Some(limits::IMAGE_SIDE);
    l.max_alloc = Some(64 * 1024 * 1024);
    l
}

fn reader(bytes: &[u8]) -> Option<ImageReader<Cursor<&[u8]>>> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(decode_limits());
    Some(reader)
}

/// Format (from the content, not the declared mime) and dimensions, from the header only.
/// `None` for anything but JPEG, PNG, WebP or GIF, or an unreadable header.
pub fn probe(bytes: &[u8]) -> Option<Info> {
    let reader = reader(bytes)?;
    let format = reader.format()?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP | ImageFormat::Gif) {
        return None;
    }
    let (width, height) = reader.into_dimensions().ok()?;
    (width > 0 && height > 0).then_some(Info { format, width, height })
}

/// Whether a WebP has an animation chunk (header only).
pub fn webp_animated(bytes: &[u8]) -> bool {
    WebPDecoder::new(Cursor::new(bytes)).is_ok_and(|d| d.has_animation())
}

/// Up to the first two frames of an animated GIF or WebP (full canvas, RGBA).
fn first_frames(bytes: &[u8], format: ImageFormat) -> Option<Vec<RgbaImage>> {
    let frames = match format {
        ImageFormat::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(bytes)).ok()?;
            decoder.set_limits(decode_limits()).ok()?;
            decoder.into_frames().take(2).collect::<std::result::Result<Vec<_>, _>>().ok()?
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes)).ok()?;
            if !decoder.has_animation() {
                return Some(vec![]);
            }
            decoder.set_limits(decode_limits()).ok()?;
            decoder.into_frames().take(2).collect::<std::result::Result<Vec<_>, _>>().ok()?
        }
        _ => return Some(vec![]),
    };
    Some(frames.into_iter().map(|f| f.into_buffer()).collect())
}

/// A centred square crop of the frame, resized to the still size, as PNG.
pub fn still_png(frame: &RgbaImage) -> Option<Vec<u8>> {
    let (w, h) = frame.dimensions();
    let side = w.min(h);
    if side == 0 {
        return None;
    }
    let square = image::imageops::crop_imm(frame, (w - side) / 2, (h - side) / 2, side, side).to_image();
    let n = limits::AVATAR_STILL_SIDE;
    let still = image::imageops::resize(&square, n, n, FilterType::Triangle);
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(still).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).ok()?;
    Some(out)
}

/// A checked profile photo.
#[derive(Debug)]
pub struct Avatar {
    pub info: Info,
    /// PNG still of the first frame, when the image is animated.
    pub still: Option<Vec<u8>>,
}

/// Validates a profile photo by decoding it: JPEG, PNG, WebP or GIF, at most
/// [`limits::IMAGE_SIDE`] on each side. Animations (two frames or more) get a still.
pub fn avatar(bytes: &[u8]) -> Option<Avatar> {
    let info = probe(bytes)?;
    if info.width > limits::IMAGE_SIDE || info.height > limits::IMAGE_SIDE {
        return None;
    }
    let frames = first_frames(bytes, info.format)?;
    if frames.len() >= 2 {
        return Some(Avatar { info, still: Some(still_png(&frames[0])?) });
    }
    if frames.is_empty() {
        // A still image (or a WebP without animation): decode it fully to validate it.
        reader(bytes)?.decode().ok()?;
    }
    Some(Avatar { info, still: None })
}

/// Runs image work on a blocking thread, one job at a time.
pub async fn run<T: Send + 'static>(state: &AppState, job: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    let _slot = state.imaging.acquire().await.map_err(internal)?;
    tokio::task::spawn_blocking(job).await.map_err(internal)
}
