//! Test inputs: codes drawn by zxing-cpp's writer, saved in the image formats the tool reads.

use std::io::Cursor;

use barcode_read::decode::{Found, Options, find_codes};
use barcode_read::{input, raster};
use image::{DynamicImage, ExtendedColorType, GrayImage, ImageEncoder, ImageFormat};
use zxingcpp::{Barcode, BarcodeFormat};

/// A code at 4 pixels per module with its quiet zone; `options` are zxing-cpp creator options.
pub fn code(format: BarcodeFormat, text: &str, options: &str) -> GrayImage {
    image(
        zxingcpp::create(format)
            .options(options)
            .from_str(text)
            .unwrap(),
    )
}

pub fn bytes_code(format: BarcodeFormat, bytes: &[u8]) -> GrayImage {
    image(zxingcpp::create(format).from_slice(bytes).unwrap())
}

fn image(code: Barcode) -> GrayImage {
    let img = code.to_image_with(&zxingcpp::write().scale(4)).unwrap();
    GrayImage::from_vec(img.width() as u32, img.height() as u32, img.data()).unwrap()
}

pub fn encode(img: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(vec![]);
    img.into().write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

/// Reads a file's bytes as the tool does: the type from the bytes, then grey, then the passes.
pub fn read(bytes: &[u8], options: &Options) -> Found {
    let page = raster::image(input::sniff(bytes).unwrap(), bytes).unwrap();
    find_codes(&page, options).unwrap()
}

/// The texts read from a PNG of `img`.
pub fn texts(img: &GrayImage) -> Vec<String> {
    let found = read(&encode(img.clone(), ImageFormat::Png), &Options::default());
    found.codes.into_iter().map(|c| c.text).collect()
}

/// `img` written by `encoder` with an Exif chunk whose only entry is the orientation tag (274).
pub fn with_exif(mut encoder: impl ImageEncoder, img: &GrayImage, orientation: u8) {
    let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
    exif.extend([orientation, 0, 0, 0, 0, 0, 0, 0]);
    encoder.set_exif_metadata(exif).unwrap();
    let (w, h) = img.dimensions();
    encoder
        .write_image(img, w, h, ExtendedColorType::L8)
        .unwrap();
}
