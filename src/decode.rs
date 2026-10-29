//! Finding codes in a grey image: the reader options, up to three passes, and the results.

use std::time::{Duration, Instant};

use base64::prelude::{BASE64_STANDARD, Engine};
use image::{GrayImage, imageops};
use zxingcpp::{
    Barcode, BarcodeFormat, BarcodeReader, Binarizer, ContentType, EanAddOnSymbol, FromStr,
    ImageFormat, ImageView, TextMode,
};

use crate::error::Error;

/// Images whose longer side is at most this get the upscaled third pass.
const UPSCALE_MAX_SIDE: u32 = 1000;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Formats and groups to look for; empty means every readable format.
    pub formats: Vec<BarcodeFormat>,
    /// One quick pass instead of up to three thorough ones.
    pub fast: bool,
}

/// One code found on a page.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequence {
    pub index: i32,
    /// `None` when the symbol doesn't say how many parts there are.
    pub count: Option<i32>,
    pub id: String,
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
pub fn find_codes(img: &GrayImage, page: usize, options: &Options) -> Result<Found, Error> {
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
    found.codes = codes.iter().map(|b| code(b, page, halve)).collect();
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
        find_codes(img, 1, &options).unwrap().passes.len()
    }

    #[test]
    fn later_passes_run_only_when_nothing_was_found() {
        let found = find_codes(&qr("one pass"), 1, &Options::default()).unwrap();
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
        };
        assert!(
            find_codes(&qr("skipped"), 1, &options)
                .unwrap()
                .codes
                .is_empty()
        );
        let options = Options {
            formats: parse_formats("all-matrix").unwrap(),
            fast: true,
        };
        assert_eq!(find_codes(&qr("kept"), 1, &options).unwrap().codes.len(), 1);
    }

    #[test]
    fn reports_raw_bytes_only_for_payloads_that_are_not_text() {
        let found = find_codes(&qr("plain"), 1, &Options::default()).unwrap();
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
        let c = &find_codes(&img, 1, &Options::default()).unwrap().codes[0];
        assert_eq!(c.content, "binary");
        assert_eq!(
            BASE64_STANDARD
                .decode(c.bytes_base64.as_ref().unwrap())
                .unwrap(),
            bytes
        );
    }
}
