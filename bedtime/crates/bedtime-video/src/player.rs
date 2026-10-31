//! Playing a file with AVFoundation: an `AVPlayer` for timing, sound and
//! seeking, and an `AVPlayerItemVideoOutput` that hands over each new frame
//! as BGRA pixels, copied into an egui image. Main thread only.
use bedtime_ui::egui;
use objc2::{AnyThread, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_av_foundation::{
    AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput, AVURLAsset,
};
use objc2_core_media::{CMTime, kCMTimeZero};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::path::Path;

pub struct Player {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
}

impl Player {
    pub fn open(path: &Path) -> Result<Self, String> {
        let main = MainThreadMarker::new().ok_or("Video plays on the main thread")?;
        let path = path.to_str().ok_or("The file's path isn't UTF-8")?;
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            let asset = AVURLAsset::URLAssetWithURL_options(&url, None);
            let item = AVPlayerItem::playerItemWithAsset(&asset, main);
            // Frames as BGRA, the format every decoder can give.
            let key: &NSString =
                &*(kCVPixelBufferPixelFormatTypeKey as *const _ as *const NSString);
            let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let format: &AnyObject = &format;
            let attributes = NSDictionary::from_slices(&[key], &[format]);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), main);
            Ok(Self {
                player,
                item,
                output,
            })
        }
    }

    /// `Ok(false)` while it's opening; why it can't play.
    pub fn ready(&self) -> Result<bool, String> {
        unsafe {
            match self.item.status() {
                AVPlayerItemStatus::ReadyToPlay => Ok(true),
                AVPlayerItemStatus::Failed => Err(self
                    .item
                    .error()
                    .map(|error| error.localizedDescription().to_string())
                    .unwrap_or_else(|| "It can't be played".into())),
                _ => Ok(false),
            }
        }
    }

    pub fn size(&self) -> [f32; 2] {
        let size = unsafe { self.item.presentationSize() };
        [size.width as f32, size.height as f32]
    }

    pub fn duration(&self) -> f64 {
        let seconds = unsafe { self.item.duration().seconds() };
        if seconds.is_finite() { seconds } else { 0.0 }
    }

    pub fn position(&self) -> f64 {
        let seconds = unsafe { self.player.currentTime().seconds() };
        if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        }
    }

    pub fn playing(&self) -> bool {
        unsafe { self.player.rate() != 0.0 }
    }

    pub fn play(&self) {
        unsafe { self.player.play() }
    }

    pub fn pause(&self) {
        unsafe { self.player.pause() }
    }

    /// Go to `seconds` exactly (the frame shown follows).
    pub fn seek(&self, seconds: f64) {
        unsafe {
            let time = CMTime::with_seconds(seconds.max(0.0), 600);
            self.player
                .seekToTime_toleranceBefore_toleranceAfter(time, kCMTimeZero, kCMTimeZero);
        }
    }

    pub fn set_volume(&self, volume: f32, muted: bool) {
        unsafe {
            self.player.setVolume(volume);
            self.player.setMuted(muted);
        }
    }

    /// The frame to show now, if it's new.
    pub fn frame(&self) -> Option<egui::ColorImage> {
        unsafe {
            let now = objc2_quartz_core::CACurrentMediaTime();
            let time = self.output.itemTimeForHostTime(now);
            if !self.output.hasNewPixelBufferForItemTime(time) {
                return None;
            }
            let buffer = self
                .output
                .copyPixelBufferForItemTime_itemTimeForDisplay(time, std::ptr::null_mut())?;
            if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) != 0 {
                return None;
            }
            let width = CVPixelBufferGetWidth(&buffer);
            let height = CVPixelBufferGetHeight(&buffer);
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            let base = CVPixelBufferGetBaseAddress(&buffer) as *const u8;
            let image = (!base.is_null() && width > 0 && height > 0).then(|| {
                let bytes = std::slice::from_raw_parts(base, stride * height);
                let mut pixels = Vec::with_capacity(width * height);
                for row in bytes.chunks_exact(stride) {
                    pixels.extend(row[..width * 4].as_chunks::<4>().0.iter().map(|bgra| {
                        egui::Color32::from_rgba_premultiplied(bgra[2], bgra[1], bgra[0], 255)
                    }));
                }
                egui::ColorImage::new([width, height], pixels)
            });
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly);
            image
        }
    }
}
