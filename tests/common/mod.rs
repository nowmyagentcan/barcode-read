//! Test inputs: codes drawn by zxing-cpp's writer, saved in the image formats the tool reads,
//! and a small PDF writer. Each test file uses part of it.
#![allow(dead_code)]

use std::io::Cursor;

use barcode_read::Options;
use barcode_read::decode::{Code, Found, find_codes};
use barcode_read::engine::{self, PageInfo};
use barcode_read::error::Warning;
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
    let page = raster::image(input::sniff(bytes).unwrap(), bytes, &mut vec![]).unwrap();
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

/// What reading a file gave: each page as (number, displayed size, dpi), the codes and warnings.
#[derive(Debug, Default)]
pub struct Read {
    pub pages: Vec<(usize, (u32, u32), Option<u32>)>,
    pub codes: Vec<Code>,
    pub warnings: Vec<Warning>,
}

/// Reads every page of a file of any type, as the tool does.
pub fn read_pages(bytes: &[u8], options: &Options) -> Read {
    let file = engine::read("test".into(), bytes.to_vec(), options);
    assert_eq!(file.error, None);
    let page = |p: &PageInfo| (p.page, (p.width, p.height), p.dpi);
    Read {
        pages: file.pages.iter().map(page).collect(),
        codes: file.codes,
        warnings: file.warnings,
    }
}

/// A QR drawn as PDF operators: a filled square of `module` points per dark module, the
/// top-left corner of its quiet zone `left` and `top` points from the top-left of a page
/// `height` points tall. The quiet zone is 4 modules wide.
pub fn vector_qr(text: &str, left: f32, top: f32, module: f32, height: f32) -> String {
    let code = zxingcpp::create(BarcodeFormat::QRCode)
        .from_str(text)
        .unwrap();
    let img = code.to_image_with(&zxingcpp::write().scale(1)).unwrap();
    let (w, data) = (img.width() as usize, img.data());
    let mut ops = String::from("0 g\n");
    for (i, _) in data.iter().enumerate().filter(|(_, v)| **v < 128) {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let bottom = height - top - (y + 1.0) * module;
        ops += &format!("{} {bottom} {module} {module} re\n", left + x * module);
    }
    ops + "f\n"
}

/// A PDF with one page per (width, height, content stream), sizes in points, and `trailer`
/// added to its trailer dictionary.
pub fn pdf_file(pages: &[(f32, f32, String)], trailer: &str) -> Vec<u8> {
    let kids: Vec<_> = (0..pages.len())
        .map(|i| format!("{} 0 R", 3 + 2 * i))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        ),
    ];
    for (i, (w, h, ops)) in pages.iter().enumerate() {
        let content = 4 + 2 * i;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {content} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{ops}\nendstream",
            ops.len() + 1
        ));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = vec![];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out += &format!("{} 0 obj\n{object}\nendobj\n", i + 1);
    }
    let xref = out.len();
    out += &format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        out += &format!("{offset:010} 00000 n \n");
    }
    out += &format!(
        "trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    );
    out.into_bytes()
}
