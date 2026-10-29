//! Test inputs: codes drawn by zxing-cpp's writer, saved in the image formats the tool reads.

use std::io::Cursor;

use barcode_read::decode::{Found, Options, find_codes};
use barcode_read::{input, raster};
use image::{DynamicImage, GrayImage, ImageFormat};
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
    let img = raster::grey(input::sniff(bytes).unwrap(), bytes).unwrap();
    find_codes(&img, 1, options).unwrap()
}

/// The texts read from a PNG of `img`.
pub fn texts(img: &GrayImage) -> Vec<String> {
    let found = read(&encode(img.clone(), ImageFormat::Png), &Options::default());
    found.codes.into_iter().map(|c| c.text).collect()
}
