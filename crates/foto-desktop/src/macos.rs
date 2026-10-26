use objc2_av_foundation::{AVAssetImageGenerator, AVURLAsset};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFMutableData, CFNumber, CFString, CFType, CFURL, CGSize,
};
use objc2_core_graphics::CGImage;
use objc2_core_media::CMTime;
use objc2_foundation::{NSString, NSURL};
use objc2_image_io::{
    CGImageDestination, CGImageSource, kCGImageDestinationLossyCompressionQuality,
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
    kCGImageSourceThumbnailMaxPixelSize,
};

pub fn thumbnail(path: &str, video: bool, max_px: u32) -> Option<Vec<u8>> {
    if video {
        poster(path, max_px)
    } else {
        still(path, max_px)
    }
}

fn still(path: &str, max_px: u32) -> Option<Vec<u8>> {
    let url = CFURL::from_file_path(path)?;
    let src = unsafe { CGImageSource::with_url(&url, None) }?;
    let max = CFNumber::new_i32(max_px as i32);
    let yes = CFBoolean::new(true);
    let keys: [&CFString; 3] = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceThumbnailMaxPixelSize,
        ]
    };
    let values: [&CFType; 3] = [yes.as_ref(), yes.as_ref(), max.as_ref()];
    let opts = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    let image = unsafe { src.thumbnail_at_index(0, Some(opts.as_ref())) }?;
    encode(&image)
}

#[allow(deprecated)]
fn poster(path: &str, max_px: u32) -> Option<Vec<u8>> {
    unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
        let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        generator.setAppliesPreferredTrackTransform(true);
        generator.setMaximumSize(CGSize {
            width: max_px as f64,
            height: max_px as f64,
        });
        let time = CMTime::with_seconds(0.5, 600);
        let image = generator
            .copyCGImageAtTime_actualTime_error(time, std::ptr::null_mut())
            .ok()?;
        encode(&image)
    }
}

fn encode(image: &CGImage) -> Option<Vec<u8>> {
    let data = CFMutableData::new(None, 0)?;
    let jpeg = CFString::from_str("public.jpeg");
    let dest = unsafe { CGImageDestination::with_data(&data, &jpeg, 1, None) }?;
    let quality = CFNumber::new_f64(0.8);
    let key: &CFString = unsafe { kCGImageDestinationLossyCompressionQuality };
    let props = CFDictionary::<CFString, CFType>::from_slices(&[key], &[quality.as_ref()]);
    unsafe {
        dest.add_image(image, Some(props.as_ref()));
        dest.finalize().then(|| data.to_vec())
    }
}
