//! HEIC decoding on macOS with Apple's own ImageIO framework.
//!
//! macOS decodes HEIC natively, so the app needs no bundled library there
//! (and a signed app could not load Homebrew's libheif anyway). This file
//! calls the plain C API of ImageIO, CoreGraphics and CoreFoundation
//! directly; those functions have been stable since macOS 10.13.
//!
//! How a file is decoded:
//! 1. `CGImageSourceCreateWithData` opens the file's bytes.
//! 2. `CGImageSourceCreateThumbnailAtIndex` with "create from image always"
//!    and "with transform" returns the image **upright** (rotation applied).
//!    The maximum size we ask for is larger than any photo, so the result is
//!    full size.
//! 3. The image is drawn into an sRGB bitmap we own, which converts wide-gamut
//!    (Display P3) iPhone photos to sRGB, and gives us plain RGBX bytes.

use std::ffi::c_void;
use std::ptr;

use image::RgbImage;

/// Any CoreFoundation object (`CFTypeRef`). CoreGraphics and ImageIO objects
/// are CoreFoundation objects too.
type Ref = *const c_void;

/// Stands for C structs we only ever take the address of.
#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// `kCFNumberSInt32Type`
const CF_NUMBER_SINT32: isize = 3;
/// `kCGImageAlphaNoneSkipLast`: 8-bit R, G, B and one unused byte per pixel.
const RGBX: u32 = 5;
/// Larger than any image, so "thumbnail" means "full size, upright".
const MAX_PIXEL_SIZE: i32 = 65_535;

#[allow(non_upper_case_globals)]
#[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
extern "C" {
    static kCFBooleanTrue: Ref;
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;

    fn CFRelease(object: Ref);
    fn CFDataCreate(allocator: Ref, bytes: *const u8, length: isize) -> Ref;
    fn CFNumberCreate(allocator: Ref, number_type: isize, value: *const c_void) -> Ref;
    fn CFDictionaryCreate(
        allocator: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> Ref;
}

#[allow(non_upper_case_globals)]
#[cfg_attr(target_os = "macos", link(name = "ImageIO", kind = "framework"))]
extern "C" {
    static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
    static kCGImageSourceCreateThumbnailWithTransform: Ref;
    static kCGImageSourceThumbnailMaxPixelSize: Ref;

    fn CGImageSourceCreateWithData(data: Ref, options: Ref) -> Ref;
    fn CGImageSourceCreateThumbnailAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
}

#[allow(non_upper_case_globals)]
#[cfg_attr(target_os = "macos", link(name = "CoreGraphics", kind = "framework"))]
extern "C" {
    static kCGColorSpaceSRGB: Ref;

    fn CGImageGetWidth(image: Ref) -> usize;
    fn CGImageGetHeight(image: Ref) -> usize;
    fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        color_space: Ref,
        bitmap_info: u32,
    ) -> Ref;
    fn CGContextDrawImage(context: Ref, rect: CGRect, image: Ref);
}

/// Releases a CoreFoundation object when dropped, so every early return
/// below cleans up after itself.
struct Owned(Ref);

impl Owned {
    /// Wraps a newly created object; `None` if creating it failed.
    fn new(object: Ref) -> Option<Self> {
        (!object.is_null()).then_some(Self(object))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a non-null object we own one reference to.
        unsafe { CFRelease(self.0) }
    }
}

/// Decodes a HEIC/HEIF file's bytes into upright sRGB pixels.
pub fn decode(bytes: &[u8]) -> Result<RgbImage, String> {
    // SAFETY: every pointer passed below is either a live local, a constant
    // exported by the framework, or an object checked for null by
    // `Owned::new`. Null allocators mean "the default allocator".
    unsafe {
        let data = Owned::new(CFDataCreate(
            ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
        ))
        .ok_or("could not wrap the file data")?;
        let source = Owned::new(CGImageSourceCreateWithData(data.0, ptr::null()))
            .ok_or("not an image ImageIO can read")?;

        let max_size = Owned::new(CFNumberCreate(
            ptr::null(),
            CF_NUMBER_SINT32,
            (&MAX_PIXEL_SIZE as *const i32).cast(),
        ))
        .ok_or("could not create a number")?;
        let keys = [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceThumbnailMaxPixelSize,
        ];
        let values = [kCFBooleanTrue, kCFBooleanTrue, max_size.0];
        let options = Owned::new(CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        ))
        .ok_or("could not create the options")?;

        let image = Owned::new(CGImageSourceCreateThumbnailAtIndex(source.0, 0, options.0))
            .ok_or("could not decode the image")?;
        let width = CGImageGetWidth(image.0);
        let height = CGImageGetHeight(image.0);
        if width == 0 || height == 0 || width > 65_535 || height > 65_535 {
            return Err(format!("unexpected image size {width}x{height}"));
        }

        // Draw into our own RGBX buffer. Bitmap memory starts with the top row.
        let mut rgbx = vec![0u8; width * height * 4];
        let color_space = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))
            .ok_or("could not create the sRGB color space")?;
        let context = Owned::new(CGBitmapContextCreate(
            rgbx.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            color_space.0,
            RGBX,
        ))
        .ok_or("could not create a bitmap")?;
        let whole = CGRect {
            x: 0.0,
            y: 0.0,
            width: width as f64,
            height: height as f64,
        };
        CGContextDrawImage(context.0, whole, image.0);
        // The context writes into `rgbx`; let go of it before reading.
        drop(context);

        let mut rgb = Vec::with_capacity(width * height * 3);
        for pixel in rgbx.as_chunks::<4>().0 {
            rgb.extend_from_slice(&pixel[..3]);
        }
        RgbImage::from_raw(width as u32, height as u32, rgb)
            .ok_or_else(|| "unexpected pixel data size".to_string())
    }
}
