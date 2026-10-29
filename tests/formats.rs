//! Codes written by zxing-cpp read back through the engine: every writable format, the payload
//! kinds, the image formats and pixel layouts, and the hard cases the extra passes exist for.

mod common;

use barcode_read::Options;
use barcode_read::decode::{Page, find_codes};
use barcode_read::{input::Kind, raster};
use base64::prelude::{BASE64_STANDARD, Engine};
use common::{bytes_code, code, encode, read, texts, with_exif};
use image::codecs::{jpeg::JpegEncoder, png::PngEncoder, webp::WebPEncoder};
use image::{GrayImage, ImageBuffer, ImageFormat, Luma, LumaA, imageops, metadata::Orientation};
use zxingcpp::BarcodeFormat::{self, *};

fn png(img: &GrayImage) -> Vec<u8> {
    encode(img.clone(), ImageFormat::Png)
}

#[test]
fn reads_back_every_writable_format() {
    let cases: [(BarcodeFormat, &str, &str, &str); 19] = [
        (
            QRCode,
            "https://ex.co/i/pZVdc8iMu0",
            "QR Code",
            "https://ex.co/i/pZVdc8iMu0",
        ),
        (MicroQRCode, "12345", "Micro QR Code", "12345"),
        (RMQRCode, "RMQR TEST 123", "rMQR Code", "RMQR TEST 123"),
        (
            DataMatrix,
            "Hello DataMatrix",
            "Data Matrix",
            "Hello DataMatrix",
        ),
        (
            PDF417,
            "PDF417 payload 1234567890",
            "PDF417",
            "PDF417 payload 1234567890",
        ),
        (Aztec, "Aztec ticket 42", "Aztec", "Aztec ticket 42"),
        (MaxiCode, "MaxiCode 123456", "MaxiCode", "MaxiCode 123456"),
        (EAN13, "9780201379624", "EAN-13", "9780201379624"),
        (EAN8, "96385074", "EAN-8", "96385074"),
        (UPCA, "036000291452", "EAN-13", "0036000291452"),
        (UPCE, "01234565", "UPC-E", "0012345000065"),
        (Code128, "ABC-123-xyz", "Code 128", "ABC-123-xyz"),
        (Code39, "CODE39 TEST", "Code 39", "CODE39 TEST"),
        (Code93, "CODE93", "Code 93", "CODE93"),
        (Codabar, "A40156B", "Codabar", "A40156B"),
        (ITF, "12345678901231", "ITF", "12345678901231"),
        (
            DataBar,
            "0950110153000",
            "DataBar Omni",
            "(01)09501101530003",
        ),
        (
            DataBarLtd,
            "0950110153000",
            "DataBar Limited",
            "(01)09501101530003",
        ),
        (
            DataBarExp,
            "(01)09501101530003(3103)000123",
            "DataBar Expanded",
            "(01)09501101530003(3103)000123",
        ),
    ];
    for (format, input, name, text) in cases {
        let found = read(&png(&code(format, input, "")), &Options::default());
        let got: Vec<_> = found
            .codes
            .iter()
            .map(|c| (c.format.as_str(), c.text.as_str()))
            .collect();
        assert_eq!(got, [(name, text)], "{format:?}");
    }
}

#[test]
fn keeps_binary_gs1_and_unicode_payloads_intact() {
    let bytes: Vec<u8> = (0..=255).collect();
    let found = read(&png(&bytes_code(QRCode, &bytes)), &Options::default());
    let c = &found.codes[0];
    assert_eq!(c.content, "binary");
    assert_eq!(
        BASE64_STANDARD
            .decode(c.bytes_base64.as_ref().unwrap())
            .unwrap(),
        bytes
    );

    let gs1 = "(01)09501101530003(17)261231(10)ABC123";
    let found = read(&png(&code(DataMatrix, gs1, "gs1")), &Options::default());
    let c = &found.codes[0];
    assert_eq!(
        (c.content, c.text.as_str(), c.symbology_id.as_str()),
        ("gs1", gs1, "]d2")
    );
    let raw = BASE64_STANDARD
        .decode(c.bytes_base64.as_ref().unwrap())
        .unwrap();
    assert_eq!(raw, b"01095011015300031726123110ABC123");

    // A group separator ends a variable-length field that isn't the last one.
    let found = read(
        &png(&code(DataMatrix, "(10)ABC(17)261231", "gs1")),
        &Options::default(),
    );
    let raw = BASE64_STANDARD
        .decode(found.codes[0].bytes_base64.as_ref().unwrap())
        .unwrap();
    assert_eq!(raw, b"10ABC\x1D17261231");

    let found = read(
        &png(&code(Code128, "(01)09501101530003(10)LOT7", "gs1")),
        &Options::default(),
    );
    let c = &found.codes[0];
    assert_eq!((c.content, c.symbology_id.as_str()), ("gs1", "]C1"));
    assert_eq!(c.text, "(01)09501101530003(10)LOT7");

    for text in [
        "BEGIN:VCARD\nVERSION:3.0\nFN:Ann Lee\nEND:VCARD",
        "Grüße 東京 ✓",
    ] {
        assert_eq!(texts(&code(QRCode, text, "")), [text]);
    }
}

#[test]
fn reports_each_structured_append_part() {
    // A four-part QR sequence from zxing-cpp's own test images (Apache-2.0).
    let path = "/vendor/zxing-cpp/test/samples/qrcode-2/StructApp.webp";
    let bytes = std::fs::read(env!("CARGO_MANIFEST_DIR").to_owned() + path).unwrap();
    let found = read(&bytes, &Options::default());
    let mut parts: Vec<_> = found
        .codes
        .iter()
        .map(|c| c.sequence.clone().unwrap())
        .collect();
    parts.sort_by_key(|s| s.index);
    for (i, s) in parts.iter().enumerate() {
        assert_eq!((s.index, s.count, s.id.as_str()), (i as i32, Some(4), "95"));
    }
    assert_eq!(parts.len(), 4);
}

/// `parts` pasted onto a white canvas at their top-left corners.
fn canvas(width: u32, height: u32, parts: &[(&GrayImage, i64, i64)]) -> GrayImage {
    let mut img = GrayImage::from_pixel(width, height, Luma([255]));
    for (part, x, y) in parts {
        imageops::replace(&mut img, *part, *x, *y);
    }
    img
}

#[test]
fn sorts_codes_top_to_bottom_then_left_to_right() {
    let qr = code(QRCode, "qr", "");
    let dm = code(DataMatrix, "dm", "");
    let c128 = code(Code128, "c128", "");
    let img = canvas(700, 700, &[(&qr, 20, 60), (&dm, 400, 20), (&c128, 20, 400)]);
    assert_eq!(texts(&img), ["dm", "qr", "c128"]);
    let (left, right) = (code(QRCode, "left", ""), code(QRCode, "right", ""));
    assert_eq!(
        texts(&canvas(500, 200, &[(&right, 250, 0), (&left, 0, 0)])),
        ["left", "right"]
    );
}

#[test]
fn reads_rotated_inverted_and_transparent_codes() {
    let c128 = code(Code128, "ABC-123-xyz", "");
    for rotated in [
        imageops::rotate90(&c128),
        imageops::rotate180(&c128),
        imageops::rotate270(&c128),
    ] {
        assert_eq!(texts(&rotated), ["ABC-123-xyz"]);
    }
    let mut negative = code(QRCode, "white on black", "");
    imageops::invert(&mut negative);
    assert_eq!(texts(&negative), ["white on black"]);

    let qr = code(QRCode, "black on transparent", "");
    let ink = ImageBuffer::from_fn(qr.width(), qr.height(), |x, y| {
        LumaA([0, 255 - qr[(x, y)].0[0]])
    });
    let found = read(&encode(ink, ImageFormat::Png), &Options::default());
    assert_eq!(found.codes[0].text, "black on transparent");
}

#[test]
fn reads_every_image_format_and_sample_depth() {
    let qr = code(QRCode, "every format", "");
    let deep = ImageBuffer::from_fn(qr.width(), qr.height(), |x, y| {
        Luma([u16::from(qr[(x, y)].0[0]) * 257])
    });
    let files = [
        encode(deep, ImageFormat::Png),
        encode(
            image::DynamicImage::ImageLuma8(qr.clone()).into_rgba8(),
            ImageFormat::Gif,
        ),
        encode(qr.clone(), ImageFormat::Bmp),
        encode(qr.clone(), ImageFormat::WebP),
        pgm(&qr),
        encode(qr.clone(), ImageFormat::Jpeg),
    ];
    for file in files {
        let found = read(&file, &Options::default());
        assert_eq!(found.codes[0].text, "every format", "{:?}", &file[..4]);
    }
}

#[test]
fn reports_an_exif_turned_photo_as_displayed() {
    let stored = canvas(300, 200, &[(&code(QRCode, "turned", ""), 20, 10)]);
    let (mut jpeg, mut png, mut webp) = (vec![], vec![], vec![]);
    with_exif(JpegEncoder::new(&mut jpeg), &stored, 6);
    with_exif(PngEncoder::new(&mut png), &stored, 6);
    with_exif(WebPEncoder::new_lossless(&mut webp), &stored, 6);
    let shown = Page::new(1, imageops::rotate90(&stored));
    let expected = &find_codes(&shown, &Options::default()).unwrap().codes[0];
    for (kind, bytes) in [(Kind::Jpeg, jpeg), (Kind::Png, png), (Kind::Webp, webp)] {
        let page = raster::image(kind, &bytes, &mut vec![]).unwrap();
        assert_eq!(page.orientation, Orientation::Rotate90, "{kind:?}");
        assert_eq!(page.size(), (200, 300));
        let c = &find_codes(&page, &Options::default()).unwrap().codes[0];
        assert_eq!((c.text.as_str(), c.rotation), ("turned", expected.rotation));
        for (got, want) in c.position.iter().zip(&expected.position) {
            assert!(
                (got[0] - want[0]).abs() <= 2 && (got[1] - want[1]).abs() <= 2,
                "{kind:?}: {:?} vs {:?}",
                c.position,
                expected.position
            );
        }
    }
}

#[test]
fn reads_a_shrunk_code_128_through_the_upscaled_pass() {
    let c128 = code(Code128, "ABC-123-xyz", "");
    assert_eq!(c128.dimensions(), (704, 200));
    let small = imageops::resize(&c128, 281, 80, imageops::FilterType::Triangle);
    let found = read(&png(&small), &Options::default());
    assert_eq!(found.passes.len(), 3, "passes 1 and 2 must miss it");
    let c = &found.codes[0];
    assert_eq!(c.text, "ABC-123-xyz");
    assert!(
        c.position
            .iter()
            .all(|&[x, y]| (0..281).contains(&x) && (0..80).contains(&y)),
        "{:?}",
        c.position
    );
}

/// A binary PGM (P5): `image` writes PAM (P7) unless told otherwise.
fn pgm(img: &GrayImage) -> Vec<u8> {
    use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
    let mut bytes = vec![];
    let encoder =
        PnmEncoder::new(&mut bytes).with_subtype(PnmSubtype::Graymap(SampleEncoding::Binary));
    img.write_with_encoder(encoder).unwrap();
    bytes
}

/// A two-frame animation in each format, "first" then "second": only frame 1 is read.
#[test]
fn reads_the_first_frame_of_an_animation() {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame};
    let frames = ["first", "second"]
        .map(|t| image::DynamicImage::ImageLuma8(code(QRCode, t, "")).into_rgba8());
    let mut gif = vec![];
    GifEncoder::new(&mut gif)
        .encode_frames(
            frames
                .iter()
                .map(|f| Frame::from_parts(f.clone(), 0, 0, Delay::from_numer_denom_ms(100, 1))),
        )
        .unwrap();
    let webp = animated_webp(&frames.clone().map(|f| encode(f, ImageFormat::WebP)));
    for (kind, bytes) in [(Kind::Gif, gif), (Kind::Webp, webp)] {
        let mut warnings = vec![];
        let page = raster::image(kind, &bytes, &mut warnings).unwrap();
        let codes = find_codes(&page, &Options::default()).unwrap().codes;
        assert_eq!(codes[0].text, "first", "{kind:?}");
        assert_eq!(warnings, [barcode_read::error::Warning::first_frame_only()]);
    }
    let mut warnings = vec![];
    raster::image(
        Kind::Gif,
        &encode(frames[0].clone(), ImageFormat::Gif),
        &mut warnings,
    )
    .unwrap();
    raster::image(
        Kind::Webp,
        &encode(frames[0].clone(), ImageFormat::WebP),
        &mut warnings,
    )
    .unwrap();
    assert!(warnings.is_empty());
}

/// An animated WebP whose frames are the image data of still lossless WebPs of one size.
fn animated_webp(stills: &[Vec<u8>]) -> Vec<u8> {
    let chunk = |name: &[u8], data: &[u8]| {
        let mut out = [name, &(data.len() as u32).to_le_bytes(), data].concat();
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    };
    // A lossless still's VP8L header packs width - 1 and height - 1 into 14 bits each.
    let bits = u32::from_le_bytes(stills[0][21..25].try_into().unwrap());
    let (w1, h1) = (bits & 0x3FFF, (bits >> 14) & 0x3FFF);
    let u24 = |v: u32| v.to_le_bytes()[..3].to_vec();
    let mut body = b"WEBP".to_vec();
    body.extend(chunk(
        b"VP8X",
        &[[2, 0, 0, 0].as_slice(), &u24(w1), &u24(h1)].concat(),
    ));
    body.extend(chunk(b"ANIM", &[255, 255, 255, 255, 0, 0]));
    for still in stills {
        let frame = [
            u24(0),
            u24(0),
            u24(w1),
            u24(h1),
            u24(100),
            vec![0],
            still[12..].to_vec(),
        ]
        .concat();
        body.extend(chunk(b"ANMF", &frame));
    }
    chunk(b"RIFF", &body)
}
