//! Finding codes in a grey image: the reader options, up to three passes, and the results.

use std::time::{Duration, Instant};

use base64::prelude::{BASE64_STANDARD, Engine};
use image::{GrayImage, imageops, metadata::Orientation};
use schemars::JsonSchema;
use serde::Serialize;
use zxingcpp::{
    Barcode, BarcodeFormat, BarcodeReader, Binarizer, ContentType, EanAddOnSymbol, FromStr,
    ImageFormat, ImageView, TextMode,
};

use crate::Options;
use crate::error::Error;

/// Images whose longer side is at most this get the upscaled third pass.
const UPSCALE_MAX_SIDE: u32 = 1000;

/// One code found on a page.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Code {
    pub page: usize,
    /// zxing-cpp's name of the variant read, e.g. `QR Code`, `EAN-13`.
    pub format: String,
    /// Human-readable text: GS1 as `(AI)value`, binary bytes escaped as `<NUL>`.
    pub text: String,
    /// `text`, `binary`, `mixed`, `gs1`, `iso15434` or `unknown_eci`.
    pub content: &'static str,
    /// The raw payload, when `content` isn't `text`.
    pub bytes_base64: Option<String>,
    /// The AIM symbology identifier, e.g. `]Q1`.
    pub symbology_id: String,
    /// Corners top-left, top-right, bottom-right, bottom-left (relative to the symbol).
    pub position: [[i32; 2]; 4],
    pub rotation: i32,
    pub ec_level: Option<String>,
    pub version: Option<String>,
    pub sequence: Option<Sequence>,
}

/// A part of a structured-append sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Sequence {
    pub index: i32,
    /// `None` when the symbol doesn't say how many parts there are.
    pub count: Option<i32>,
    pub id: String,
}

/// One image or page in 8-bit grey, ready for the decoding passes.
#[derive(Debug)]
pub struct Page {
    /// 1-based; 1 for a single image.
    pub number: usize,
    pub grey: GrayImage,
    /// How the stored pixels are turned for display (EXIF); positions are reported as displayed.
    pub orientation: Orientation,
    /// The resolution a PDF page was rendered at.
    pub dpi: Option<u32>,
}

impl Page {
    pub fn new(number: usize, grey: GrayImage) -> Self {
        Self {
            number,
            grey,
            orientation: Orientation::NoTransforms,
            dpi: None,
        }
    }

    /// Width and height as displayed.
    pub fn size(&self) -> (u32, u32) {
        use Orientation::*;
        let (w, h) = self.grey.dimensions();
        if matches!(
            self.orientation,
            Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH
        ) {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// The codes in one image, and how long each pass that ran took.
#[derive(Debug, Default)]
pub struct Found {
    pub codes: Vec<Code>,
    pub passes: Vec<Duration>,
}

/// Parses a `--formats` list: comma-separated zxing-cpp names or groups, ignoring case, spaces,
/// `-`, `_` and `/` (`qrcode`, `QR Code`, `ean13`, `all-linear`).
pub fn parse_formats(list: &str) -> Result<Vec<BarcodeFormat>, Error> {
    list.split(',')
        .map(|name| match BarcodeFormat::from_str(name.trim()) {
            Ok(f) if f != BarcodeFormat::None && f != BarcodeFormat::Invalid => Ok(f),
            _ => Err(Error::unknown_format(name.trim())),
        })
        .collect()
}

/// Reads every code on one page. Each later pass runs only when the earlier ones found nothing:
/// 1. zxing-cpp's thorough defaults; 2. the same with a global-histogram binarizer;
/// 3. pass 1 on a 2x upscale, for images up to 1000 px whose codes may be too small.
pub fn find_codes(page: &Page, options: &Options) -> Result<Found, Error> {
    let img = &page.grey;
    let mut found = Found::default();
    let mut pass = |img: &GrayImage, binarizer| {
        let start = Instant::now();
        let codes = read(img, &reader(options, binarizer));
        found.passes.push(start.elapsed());
        codes
    };
    let mut codes = pass(img, Binarizer::LocalAverage)?;
    let mut halve = false;
    if codes.is_empty() && !options.fast {
        codes = pass(img, Binarizer::GlobalHistogram)?;
        let (w, h) = img.dimensions();
        if codes.is_empty() && w.max(h) <= UPSCALE_MAX_SIDE {
            let big = imageops::resize(img, w * 2, h * 2, imageops::FilterType::Triangle);
            codes = pass(&big, Binarizer::LocalAverage)?;
            halve = true;
        }
    }
    found.codes = codes.iter().map(|b| code(b, page.number, halve)).collect();
    if page.orientation != Orientation::NoTransforms {
        let (w, h) = img.dimensions();
        for c in &mut found.codes {
            c.position = c
                .position
                .map(|p| turn(page.orientation, p, w as i32, h as i32));
            c.rotation = rotation(&c.position);
        }
    }
    found.codes.sort_by_key(|c| {
        let min = |axis: usize| c.position.iter().map(|p| p[axis]).min();
        (min(1), min(0))
    });
    Ok(found)
}

fn reader(options: &Options, binarizer: Binarizer) -> BarcodeReader {
    let thorough = !options.fast;
    zxingcpp::read()
        .formats(options.formats.as_slice())
        .try_harder(thorough)
        .try_rotate(thorough)
        .try_invert(thorough)
        .try_downscale(true)
        .binarizer(binarizer)
        .text_mode(TextMode::HRI)
        .min_line_count(2)
        .max_number_of_symbols(255)
        .validate_optional_checksum(false)
        .ean_add_on_symbol(EanAddOnSymbol::Read)
}

fn read(img: &GrayImage, reader: &BarcodeReader) -> Result<Vec<Barcode>, Error> {
    let (w, h) = img.dimensions();
    let view =
        ImageView::from_slice(img.as_raw(), w, h, ImageFormat::Lum).map_err(Error::internal)?;
    let codes = reader.from(view).map_err(Error::internal)?;
    Ok(codes.into_iter().filter(Barcode::is_valid).collect())
}

/// `halve` maps corners from the 2x upscale back: the upscale samples source pixel x at
/// x/2 - 1/4, which rounds to x/2 rounded down.
fn code(b: &Barcode, page: usize, halve: bool) -> Code {
    let p = b.position();
    let s = if halve { 2 } else { 1 };
    let corner = |p: zxingcpp::PointI| [p.x.div_euclid(s), p.y.div_euclid(s)];
    let content = match b.content_type() {
        ContentType::Text => "text",
        ContentType::Binary => "binary",
        ContentType::Mixed => "mixed",
        ContentType::GS1 => "gs1",
        ContentType::ISO15434 => "iso15434",
        ContentType::UnknownECI => "unknown_eci",
    };
    let extra = |key| Some(b.extra_with_key(key)).filter(|v| !v.is_empty());
    let (index, count) = (b.sequence_index(), b.sequence_size());
    Code {
        page,
        format: b.format().to_string(),
        text: b.text(),
        content,
        bytes_base64: (content != "text").then(|| BASE64_STANDARD.encode(b.bytes())),
        symbology_id: b.symbology_identifier(),
        position: [p.top_left, p.top_right, p.bottom_right, p.bottom_left].map(corner),
        rotation: b.rotation(),
        ec_level: extra("ECLevel"),
        version: extra("Version"),
        sequence: (index >= 0 && count >= 0).then(|| Sequence {
            index,
            count: (count > 0).then_some(count),
            id: b.sequence_id(),
        }),
    }
}

/// Where a corner of a `w` x `h` stored image shows once its EXIF orientation is applied.
/// zxing-cpp's corners sit on pixel edges, so a mirror maps x to w - x (kept inside the image).
fn turn(o: Orientation, [x, y]: [i32; 2], w: i32, h: i32) -> [i32; 2] {
    use Orientation::*;
    let (right, bottom) = ((w - x).clamp(0, w - 1), (h - y).clamp(0, h - 1));
    match o {
        NoTransforms => [x, y],
        FlipHorizontal => [right, y],
        Rotate180 => [right, bottom],
        FlipVertical => [x, bottom],
        Rotate90FlipH => [y, x],
        Rotate90 => [bottom, x],
        Rotate270FlipH => [bottom, right],
        Rotate270 => [y, right],
    }
}

/// zxing-cpp's rotation: the angle of the line from the left edge's middle to the right edge's.
fn rotation([tl, tr, br, bl]: &[[i32; 2]; 4]) -> i32 {
    let [x, y] = [0, 1].map(|i| f64::from(tr[i] + br[i] - tl[i] - bl[i]));
    y.atan2(x).to_degrees().round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_ignore_case_spaces_and_separators() {
        let parsed = parse_formats("qrcode, QR Code,Data_Matrix,ean-13,code128,UPC/A").unwrap();
        use BarcodeFormat::*;
        assert_eq!(parsed, [QRCode, QRCode, DataMatrix, EAN13, Code128, UPCA]);
    }

    #[test]
    fn format_groups_are_names_too() {
        let groups = "all-linear,All Matrix,all_gs1,allretail,ALL-INDUSTRIAL";
        use BarcodeFormat::*;
        assert_eq!(
            parse_formats(groups).unwrap(),
            [AllLinear, AllMatrix, AllGS1, AllRetail, AllIndustrial]
        );
    }

    #[test]
    fn an_unknown_format_names_the_bad_entry() {
        let e = parse_formats("qrcode, qr").unwrap_err();
        assert_eq!(e.code, "unknown_format");
        assert_eq!(e.message, "Unknown barcode format 'qr'.");
        assert_eq!(
            e.hint,
            Some("Use names like qrcode, datamatrix, ean13, code128, all-linear or all-matrix.")
        );
        for bad in ["", "qrcode,", "none", "qr\0code"] {
            assert_eq!(
                parse_formats(bad).unwrap_err().code,
                "unknown_format",
                "{bad:?}"
            );
        }
    }

    fn image(code: zxingcpp::Barcode) -> GrayImage {
        let img = code.to_image_with(&zxingcpp::write().scale(4)).unwrap();
        GrayImage::from_vec(img.width() as u32, img.height() as u32, img.data()).unwrap()
    }

    fn qr(text: &str) -> GrayImage {
        image(
            zxingcpp::create(BarcodeFormat::QRCode)
                .from_str(text)
                .unwrap(),
        )
    }

    fn passes(img: &GrayImage, fast: bool) -> usize {
        let options = Options {
            fast,
            ..Options::default()
        };
        find_codes(&Page::new(1, img.clone()), &options)
            .unwrap()
            .passes
            .len()
    }

    #[test]
    fn later_passes_run_only_when_nothing_was_found() {
        let found = find_codes(&Page::new(1, qr("one pass")), &Options::default()).unwrap();
        assert_eq!(
            (found.codes[0].text.as_str(), found.passes.len()),
            ("one pass", 1)
        );
        assert_eq!(passes(&GrayImage::new(1000, 40), false), 3);
        assert_eq!(passes(&GrayImage::new(1001, 40), false), 2);
        assert_eq!(passes(&GrayImage::new(40, 40), true), 1);
    }

    #[test]
    fn only_the_chosen_formats_are_read() {
        let options = Options {
            formats: parse_formats("datamatrix").unwrap(),
            fast: true,
            ..Options::default()
        };
        assert!(
            find_codes(&Page::new(1, qr("skipped")), &options)
                .unwrap()
                .codes
                .is_empty()
        );
        let options = Options {
            formats: parse_formats("all-matrix").unwrap(),
            fast: true,
            ..Options::default()
        };
        assert_eq!(
            find_codes(&Page::new(1, qr("kept")), &options)
                .unwrap()
                .codes
                .len(),
            1
        );
    }

    #[test]
    fn reports_raw_bytes_only_for_payloads_that_are_not_text() {
        let found = find_codes(&Page::new(1, qr("plain")), &Options::default()).unwrap();
        let c = &found.codes[0];
        assert_eq!((c.content, c.bytes_base64.as_deref()), ("text", None));
        assert_eq!(
            (c.symbology_id.as_str(), c.rotation, c.sequence.as_ref()),
            ("]Q1", 0, None)
        );
        assert!(
            c.ec_level.is_some() && c.version.as_deref() == Some("1"),
            "{c:?}"
        );

        let bytes: Vec<u8> = (0..=255).collect();
        let img = image(
            zxingcpp::create(BarcodeFormat::QRCode)
                .from_slice(&bytes)
                .unwrap(),
        );
        let c = &find_codes(&Page::new(1, img), &Options::default())
            .unwrap()
            .codes[0];
        assert_eq!(c.content, "binary");
        assert_eq!(
            BASE64_STANDARD
                .decode(c.bytes_base64.as_ref().unwrap())
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn positions_follow_the_displayed_orientation() {
        use Orientation::*;
        // A QR off-centre on a wide canvas, so every turn and flip moves it somewhere else.
        let mut stored = GrayImage::from_pixel(300, 200, image::Luma([255]));
        imageops::replace(&mut stored, &qr("turned"), 20, 10);
        let turns = [Rotate90, Rotate180, Rotate270];
        let flips = [FlipHorizontal, FlipVertical, Rotate90FlipH, Rotate270FlipH];
        for o in turns.into_iter().chain(flips) {
            let page = Page {
                orientation: o,
                ..Page::new(1, stored.clone())
            };
            let mapped = &find_codes(&page, &Options::default()).unwrap().codes[0];
            let mut shown = image::DynamicImage::ImageLuma8(stored.clone());
            shown.apply_orientation(o);
            assert_eq!(page.size(), shown.to_luma8().dimensions(), "{o:?}");
            let seen = &find_codes(&Page::new(1, shown.to_luma8()), &Options::default())
                .unwrap()
                .codes[0];
            // Corner estimates differ by a pixel or two between the two images. zxing-cpp names
            // the corners of a mirrored symbol differently, so for flips only the points count.
            let (m, s) = (mapped.position, seen.position);
            let near =
                |a: &[i32; 2], b: &[i32; 2]| (a[0] - b[0]).abs() <= 2 && (a[1] - b[1]).abs() <= 2;
            let close = if flips.contains(&o) {
                m.iter().all(|a| s.iter().any(|b| near(a, b)))
            } else {
                let turn = (mapped.rotation - seen.rotation).rem_euclid(360);
                (turn <= 1 || turn >= 359) && m.iter().zip(&s).all(|(a, b)| near(a, b))
            };
            assert!(close, "{o:?}: {m:?} vs {s:?}");
        }
    }
}
