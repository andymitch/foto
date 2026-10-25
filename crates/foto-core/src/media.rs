use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Local, NaiveDateTime};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageReader};
use nom_exif::{ExifTag, Metadata, TrackInfoTag};

use crate::Platform;
use crate::model::Kind;

pub const SMALL: u32 = 360;
pub const LARGE: u32 = 1600;

const PHOTO: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "tif", "tiff", "heic", "heif", "avif", "dng", "cr2",
    "cr3", "nef", "arw", "raf", "orf", "rw2",
];
const VIDEO: &[&str] = &["mp4", "mov", "m4v", "3gp", "mkv", "webm", "avi"];

pub fn ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let e = ext(path);
    if PHOTO.contains(&e.as_str()) {
        Some(Kind::Photo)
    } else if VIDEO.contains(&e.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

pub fn mime_of(path: &Path) -> String {
    match ext(path).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "heic" | "heif" => "image/heic",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "3gp" => "video/3gpp",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" => "image/x-raw",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Probe {
    pub taken_at: Option<i64>,
    pub w: u32,
    pub h: u32,
    pub duration_ms: u64,
    pub camera: Option<String>,
    pub motion: bool,
}

fn naive_secs(n: NaiveDateTime) -> i64 {
    n.and_utc().timestamp()
}

pub fn local_secs(t: std::time::SystemTime) -> i64 {
    naive_secs(DateTime::<Local>::from(t).naive_local())
}

pub fn probe(path: &Path) -> Probe {
    let mut p = Probe::default();
    match nom_exif::read_metadata(path) {
        Ok(Metadata::Exif(exif)) => {
            let num = |t| {
                exif.get(t)
                    .and_then(|v| v.as_u32().or(v.as_u16().map(u32::from)))
            };
            p.taken_at = [
                ExifTag::DateTimeOriginal,
                ExifTag::CreateDate,
                ExifTag::ModifyDate,
            ]
            .into_iter()
            .find_map(|t| exif.get(t).and_then(|v| v.as_datetime()))
            .map(|d| naive_secs(d.into_naive()));
            p.w = num(ExifTag::ExifImageWidth)
                .or(num(ExifTag::ImageWidth))
                .unwrap_or(0);
            p.h = num(ExifTag::ExifImageHeight)
                .or(num(ExifTag::ImageHeight))
                .unwrap_or(0);
            if matches!(num(ExifTag::Orientation), Some(5..=8)) {
                std::mem::swap(&mut p.w, &mut p.h);
            }
            let s = |t| {
                exif.get(t)
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            };
            p.camera = match (s(ExifTag::Make), s(ExifTag::Model)) {
                (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
                (Some(make), Some(model)) => Some(format!("{make} {model}")),
                (a, b) => a.or(b),
            };
            p.motion = exif.has_embedded_track();
        }
        Ok(Metadata::Track(track)) => {
            p.taken_at = track
                .get(TrackInfoTag::CreateDate)
                .and_then(|v| v.as_datetime())
                .map(|d| match d.aware() {
                    Some(a) => naive_secs(a.with_timezone(&Local).naive_local()),
                    None => naive_secs(d.into_naive()),
                })
                .filter(|t| *t > 0);
            p.duration_ms = track
                .get(TrackInfoTag::DurationMs)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            p.w = track
                .get(TrackInfoTag::Width)
                .and_then(|v| v.as_u32())
                .unwrap_or(0);
            p.h = track
                .get(TrackInfoTag::Height)
                .and_then(|v| v.as_u32())
                .unwrap_or(0);
            let s = |t| {
                track
                    .get(t)
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            };
            p.camera = s(TrackInfoTag::Model).or(s(TrackInfoTag::Make));
        }
        Err(_) => {}
    }
    p
}

fn decode(path: &Path) -> Option<DynamicImage> {
    let mut dec = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = dec.orientation().ok()?;
    let mut img = DynamicImage::from_decoder(dec).ok()?;
    img.apply_orientation(orientation);
    Some(img)
}

fn ffmpeg_frame(path: &Path) -> Option<DynamicImage> {
    let out = Command::new("ffmpeg")
        .args(["-v", "quiet", "-ss", "0.5", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf"])
        .arg(format!(
            "scale={LARGE}:{LARGE}:force_original_aspect_ratio=decrease"
        ))
        .args(["-f", "image2pipe", "-vcodec", "png", "-"])
        .output()
        .ok()?;
    image::load_from_memory(&out.stdout).ok()
}

pub fn jpeg(img: &DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = JpegEncoder::new_with_quality(&mut out, 80).encode_image(&img.to_rgb8());
    out
}

fn placeholder() -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 3, image::Rgb([48, 48, 52])))
}

pub struct Thumbs {
    pub small: Vec<u8>,
    pub large: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub fn thumbs(platform: &dyn Platform, path: &Path, kind: Kind) -> Thumbs {
    let video = kind == Kind::Video;
    let from_platform = platform
        .thumbnail(path.to_string_lossy().into_owned(), video, LARGE)
        .and_then(|b| image::load_from_memory(&b).ok());
    let large = from_platform
        .or_else(|| {
            if video {
                ffmpeg_frame(path)
            } else {
                decode(path)
            }
        })
        .unwrap_or_else(placeholder);
    let (w, h) = (large.width(), large.height());
    let large = if w.max(h) > LARGE {
        large.thumbnail(LARGE, LARGE)
    } else {
        large
    };
    let small = large.thumbnail(SMALL, SMALL);
    Thumbs {
        small: jpeg(&small),
        large: jpeg(&large),
        w,
        h,
    }
}

pub fn same_orientation(w: u32, h: u32, tw: u32, th: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        (tw, th)
    } else if (w > h) != (tw > th) && tw != th {
        (h, w)
    } else {
        (w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a/IMG.HEIC")), Some(Kind::Photo));
        assert_eq!(kind_of(Path::new("b.mov")), Some(Kind::Video));
        assert_eq!(kind_of(Path::new("notes.txt")), None);
    }

    #[test]
    fn orientation_follows_thumb() {
        assert_eq!(same_orientation(4000, 3000, 900, 1600), (3000, 4000));
        assert_eq!(same_orientation(4000, 3000, 1600, 900), (4000, 3000));
        assert_eq!(same_orientation(0, 0, 10, 20), (10, 20));
    }
}
