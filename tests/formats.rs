//! Codes written by zxing-cpp read back through the engine: every writable format, the payload
//! kinds, the image formats and pixel layouts, and the hard cases the extra passes exist for.

mod common;

use barcode_read::Options;
use barcode_read::decode::{Page, find_codes};
use barcode_read::{engine, input::Kind, raster};
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

/// zxing-cpp issue #1150: two scan rows both find the stretched finder pattern on the left. Before
/// the fix the duplicate reached a line trace with no direction, which never ends on ARM64 and
/// gives a bogus QR result on x86-64.
#[test]
fn a_finder_pattern_found_twice_neither_hangs_nor_reads() {
    let squares = [
        (4, 5, 14, 39),
        (6, 9, 12, 35),
        (7, 11, 11, 33),
        (44, 18, 51, 25),
        (45, 19, 50, 24),
        (46, 20, 49, 23),
    ];
    // Nested squares: black, white, black.
    let img = GrayImage::from_fn(55, 42, |x, y| {
        let inside = squares
            .iter()
            .filter(|&&(x0, y0, x1, y1)| (x0..x1).contains(&x) && (y0..y1).contains(&y));
        Luma([[255, 0][inside.count() % 2]])
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let found = read(&png(&img), &Options::default()).codes.len();
        let view =
            zxingcpp::ImageView::from_slice(img.as_raw(), 55, 42, zxingcpp::ImageFormat::Lum);
        let errors = zxingcpp::read()
            .return_errors(true)
            .from(view.unwrap())
            .unwrap();
        tx.send((found, errors.len())).unwrap();
    });
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(1)),
        Ok((0, 0))
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

/// An uncompressed one-strip TIFF of `bits`-bit grey samples (0 is black), each row padded to
/// a byte; `level` gives the sample for a pixel of `img`.
fn packed_tiff(img: &GrayImage, bits: u8, level: impl Fn(u8) -> u8) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let strip = pack(img, bits, level);
    raw_tiff((w, h), bits.into(), 1, 1, &[], &[strip], &[])
}

/// `level` of each pixel of `img` in `bits` bits, packed from the high bit, rows padded to a byte.
fn pack(img: &GrayImage, bits: u8, level: impl Fn(u8) -> u8) -> Vec<u8> {
    let mut strip = vec![];
    for row in img.rows() {
        let (mut byte, mut used) = (0, 0);
        for p in row {
            byte |= level(p[0]) << (8 - bits - used);
            used += bits;
            if used == 8 {
                strip.push(byte);
                (byte, used) = (0, 0);
            }
        }
        if used > 0 {
            strip.push(byte);
        }
    }
    strip
}

/// An uncompressed one-page TIFF with `samples` samples of `bits` bits per pixel, `extra` as
/// its ExtraSamples, `tags` as more (tag, type, values) entries, and one strip per entry of
/// `strips`: several make it planar (one per plane), one is interleaved. With a TileWidth tag
/// (322), `strips` are tiles instead, plane after plane. Directory entries come in tag order
/// from offset `ifd + 2`.
fn raw_tiff(
    (w, h): (u32, u32),
    bits: u16,
    samples: u16,
    photometric: u16,
    extra: &[u16],
    strips: &[Vec<u8>],
    tags: &[(u16, u16, Vec<u32>)],
) -> Vec<u8> {
    let mut out = b"II*\0\0\0\0\0".to_vec();
    let mut offsets = vec![];
    for strip in strips {
        offsets.push(out.len() as u32);
        out.extend(strip);
    }
    out.resize(out.len().next_multiple_of(2), 0);
    let ifd = out.len() as u32;
    out[4..8].copy_from_slice(&ifd.to_le_bytes());
    let (short, long) = (3, 4);
    let mut entries: Vec<(u16, u16, Vec<u32>)> = vec![
        (256, long, vec![w]),
        (257, long, vec![h]),
        (258, short, vec![bits.into(); samples.into()]),
        (259, short, vec![1]),
        (262, short, vec![photometric.into()]),
        (277, short, vec![samples.into()]),
        (284, short, vec![if strips.len() > 1 { 2 } else { 1 }]),
    ];
    let counts = strips.iter().map(|s| s.len() as u32).collect();
    entries.extend(match tags.iter().any(|t| t.0 == 322) {
        true => vec![(324, long, offsets), (325, long, counts)],
        false => vec![
            (273, long, offsets),
            (278, long, vec![h]),
            (279, long, counts),
        ],
    });
    entries.extend(tags.iter().cloned());
    if !extra.is_empty() {
        entries.push((338, short, extra.iter().map(|&e| e.into()).collect()));
    }
    entries.sort_by_key(|e| e.0);
    // Values longer than 4 bytes go after the directory.
    let far = ifd + 2 + 12 * entries.len() as u32 + 4;
    let mut tail = vec![];
    out.extend((entries.len() as u16).to_le_bytes());
    for (tag, kind, values) in entries {
        let mut bytes: Vec<u8> = values
            .iter()
            .flat_map(|&v| match kind {
                3 => (v as u16).to_le_bytes().to_vec(),
                _ => v.to_le_bytes().to_vec(),
            })
            .collect();
        out.extend([tag.to_le_bytes(), kind.to_le_bytes()].concat());
        out.extend((values.len() as u32).to_le_bytes());
        if bytes.len() > 4 {
            out.extend((far + tail.len() as u32).to_le_bytes());
            tail.append(&mut bytes);
        } else {
            bytes.resize(4, 0);
            out.extend(bytes);
        }
    }
    out.extend(0u32.to_le_bytes());
    out.extend(tail);
    out
}

/// Interleaves 8-bit planes into pixels, written at `bits` (8 or 16) bits per sample.
fn chunky(planes: &[&[u8]], bits: u16) -> Vec<u8> {
    let pixels = planes[0].len();
    (0..pixels)
        .flat_map(|i| planes.iter().map(move |p| p[i]))
        .flat_map(|v| match bits {
            16 => (u16::from(v) * 257).to_le_bytes().to_vec(),
            _ => vec![v],
        })
        .collect()
}

/// The grey image every page of `tiff` decodes to.
fn tiff_grey(tiff: &[u8]) -> Result<Vec<u8>, String> {
    let mut grey = vec![];
    barcode_read::tiff::pages(tiff, &Options::default(), &mut vec![], |p| {
        grey.extend(p.grey.into_raw())
    })
    .map_err(|e| e.message)?;
    Ok(grey)
}

fn tiff_texts(tiff: Vec<u8>) -> (Option<String>, Vec<String>) {
    let file = engine::read("test.tif".into(), tiff, &Options::default());
    let texts = file.codes.into_iter().map(|c| c.text).collect();
    (file.error.map(|e| e.message), texts)
}

#[test]
fn a_planar_tiff_reads_like_an_interleaved_one() {
    let qr = code(QRCode, "planar", "");
    let (w, h) = qr.dimensions();
    let opaque = vec![255; qr.len()];
    for bits in [8, 16] {
        let (grey, alpha) = (chunky(&[&qr], bits), chunky(&[&opaque], bits));
        let rgb = vec![grey.clone(), grey.clone(), grey];
        let mut rgba = rgb.clone();
        rgba.push(alpha);
        for (planes, extra) in [(rgb, vec![]), (rgba, vec![2])] {
            let n = planes.len() as u16;
            let tiff = raw_tiff((w, h), bits, n, 2, &extra, &planes, &[]);
            let read = tiff_texts(tiff);
            assert_eq!(
                read,
                (Option::None, vec!["planar".into()]),
                "{bits}-bit, {n} planes"
            );
        }
    }
}

#[test]
fn a_tiled_planar_tiff_drops_the_tile_padding() {
    // 20 x 20 pixels in 16 x 16 tiles: the right and bottom tiles are padded with 0x55.
    let (w, h, t) = (20, 20, 16);
    let grey: Vec<u8> = (0..w * h).map(|i| (i * 7 % 256) as u8).collect();
    let tile = |x0: u32, y0: u32| {
        let at = |x: u32, y: u32| (x < w && y < h).then(|| grey[(y * w + x) as usize]);
        let pixels = (0..t * t).map(|i| at(x0 + i % t, y0 + i / t).unwrap_or(0x55));
        pixels.collect::<Vec<_>>()
    };
    let corners = [(0, 0), (t, 0), (0, t), (t, t)];
    let tiles: Vec<_> = (0..3)
        .flat_map(|_| corners.map(|(x, y)| tile(x, y)))
        .collect();
    let tiff = raw_tiff(
        (w, h),
        8,
        3,
        2,
        &[],
        &tiles,
        &[(322, 4, vec![t]), (323, 4, vec![t])],
    );
    assert_eq!(tiff_grey(&tiff), Ok(grey));
}

#[test]
fn a_grey_alpha_tiff_reads() {
    let qr = code(QRCode, "grey alpha", "");
    let (w, h) = qr.dimensions();
    let opaque = vec![255; qr.len()];
    for (bits, extra) in [(8, 2), (16, 2), (8, 1), (8, 0)] {
        let tiff = raw_tiff(
            (w, h),
            bits,
            2,
            1,
            &[extra],
            &[chunky(&[&qr, &opaque], bits)],
            &[],
        );
        let read = tiff_texts(tiff);
        assert_eq!(
            read,
            (Option::None, vec!["grey alpha".into()]),
            "{bits}-bit, extra {extra}"
        );
    }
    // Transparent black, opaque black and half-covered black, composited over white.
    let (grey, alpha) = ([0, 0, 0], [0, 255, 128]);
    let tiff = raw_tiff((3, 1), 8, 2, 1, &[2], &[chunky(&[&grey, &alpha], 8)], &[]);
    assert_eq!(tiff_grey(&tiff), Ok(vec![255, 0, 127]));
}

#[test]
fn associated_alpha_is_composited_once() {
    // Premultiplied: half-covered white is stored as 128, so over white it stays white.
    let (colour, alpha) = ([128, 0, 0, 255], [128, 0, 255, 255]);
    for bits in [8, 16] {
        let rgba = chunky(&[&colour, &colour, &colour, &alpha], bits);
        let tiff = raw_tiff((4, 1), bits, 4, 2, &[1], &[rgba], &[]);
        assert_eq!(tiff_grey(&tiff), Ok(vec![255, 255, 0, 255]), "{bits}-bit");
    }
    // Unassociated, the same samples are half-covered grey.
    let rgba = chunky(&[&colour, &colour, &colour, &alpha], 8);
    let tiff = raw_tiff((4, 1), 8, 4, 2, &[2], std::slice::from_ref(&rgba), &[]);
    assert_eq!(tiff_grey(&tiff), Ok(vec![191, 255, 0, 255]));
    // An unspecified extra sample isn't alpha, interleaved or planar.
    let planes = [colour, colour, colour, alpha].map(Vec::from);
    for strips in [vec![rgba], planes.to_vec()] {
        let tiff = raw_tiff((4, 1), 8, 4, 2, &[0], &strips, &[]);
        assert_eq!(tiff_grey(&tiff), Ok(vec![128, 0, 0, 255]));
    }
}

#[test]
fn a_16_bit_or_alpha_cmyk_tiff_reads() {
    let qr = code(QRCode, "cmyk", "");
    let (w, h) = qr.dimensions();
    let (none, ink) = (
        vec![0; qr.len()],
        qr.iter().map(|v| 255 - v).collect::<Vec<_>>(),
    );
    let opaque = vec![255; qr.len()];
    let cmyk16 = chunky(&[&none, &none, &none, &ink], 16);
    let cmyk16 = raw_tiff((w, h), 16, 4, 5, &[], &[cmyk16], &[]);
    let cmyka = chunky(&[&none, &none, &none, &ink, &opaque], 8);
    let cmyka = raw_tiff((w, h), 8, 5, 5, &[2], &[cmyka], &[]);
    for (name, tiff) in [("CMYK 16-bit", cmyk16), ("CMYK + alpha", cmyka)] {
        assert_eq!(
            tiff_texts(tiff),
            (Option::None, vec!["cmyk".into()]),
            "{name}"
        );
    }
}

#[test]
fn packed_grey_tiff_keeps_every_level() {
    for bits in [2, 4] {
        let max = (1u8 << bits) - 1;
        let levels = GrayImage::from_fn(u32::from(max) + 1, 1, |x, _| Luma([x as u8]));
        let mut grey = vec![];
        let tiff = packed_tiff(&levels, bits, |v| v);
        barcode_read::tiff::pages(&tiff, &Options::default(), &mut vec![], |p| {
            grey = p.grey.into_raw()
        })
        .unwrap();
        let want: Vec<_> = (0..=max)
            .map(|s| (u32::from(s) * 255 / u32::from(max)) as u8)
            .collect();
        assert_eq!(grey, want, "{bits}-bit");
    }
    // One white column more makes the width odd, so rows end inside a byte.
    let qr = code(QRCode, "packed grey", "");
    let img = canvas(qr.width() + 1, qr.height(), &[(&qr, 0, 0)]);
    for (bits, dark, light) in [(2, 2, 3), (4, 0, 15), (4, 8, 15)] {
        let tiff = packed_tiff(&img, bits, |v| if v < 128 { dark } else { light });
        let read = common::read_pages(&tiff, &Options::default());
        let texts: Vec<_> = read.codes.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            ["packed grey"],
            "{bits}-bit, levels {dark} and {light}"
        );
    }
}

#[test]
fn a_tiff_directory_that_points_to_itself_ends() {
    let mut tiff = packed_tiff(&code(QRCode, "one page", ""), 1, |v| v >> 7);
    let first = tiff[4..8].to_vec();
    let end = tiff.len();
    tiff[end - 4..].copy_from_slice(&first);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let file = engine::read("cycle.tif".into(), tiff, &Options::default());
        let texts: Vec<_> = file.codes.into_iter().map(|c| c.text).collect();
        tx.send((file.error.map(|e| e.code), texts)).unwrap();
    });
    let read = rx.recv_timeout(std::time::Duration::from_secs(5));
    assert_eq!(read, Ok((Some("corrupt_image"), vec![])));
}

#[test]
fn a_cut_short_image_is_corrupt_image() {
    let grey = code(QRCode, "cut short", "");
    // GIF has no grey encoder.
    let rgb = image::DynamicImage::from(grey.clone()).to_rgb8();
    let formats = [
        ImageFormat::Png,
        ImageFormat::Gif,
        ImageFormat::Bmp,
        ImageFormat::WebP,
    ];
    let files = formats.map(|f| (format!("{f:?}"), encode(rgb.clone(), f)));
    for (name, mut bytes) in files.into_iter().chain([("PGM".into(), pgm(&grey))]) {
        bytes.truncate(bytes.len() / 2);
        let file = engine::read("cut".into(), bytes, &Options::default());
        assert_eq!(file.error.map(|e| e.code), Some("corrupt_image"), "{name}");
    }
}

#[test]
fn a_cut_short_jpeg_is_corrupt_image() {
    let jpeg = encode(code(QRCode, "cut short", ""), ImageFormat::Jpeg);
    for percent in [10, 50, 90] {
        let bytes = jpeg[..jpeg.len() * percent / 100].to_vec();
        let file = engine::read("cut.jpg".into(), bytes, &Options::default());
        assert_eq!(
            file.error.map(|e| e.code),
            Some("corrupt_image"),
            "{percent}%"
        );
    }
}

#[test]
fn a_cut_short_jpeg_still_gives_a_code_above_the_cut() {
    let qr = code(QRCode, "above the cut", "");
    // Noise below the code holds most of the bytes, so the cut lands in it.
    let mut img = GrayImage::from_fn(qr.width(), qr.height() * 3, |x, y| {
        Luma([((x * 7919 + y * 104_729) % 251) as u8])
    });
    imageops::replace(&mut img, &qr, 0, 0);
    let jpeg = encode(img, ImageFormat::Jpeg);
    let cut = jpeg[..jpeg.len() * 3 / 4].to_vec();
    let file = engine::read("cut.jpg".into(), cut, &Options::default());
    let texts: Vec<_> = file.codes.iter().map(|c| c.text.as_str()).collect();
    let error = file.error.map(|e| e.message);
    assert_eq!((error, texts), (Option::None, vec!["above the cut"]));
}

#[test]
fn a_gif_frame_larger_than_its_canvas_is_refused() {
    // A 1x1 screen holding a 60000x60000 frame.
    let gif = b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xFF\xFF\xFF,\0\0\0\0\x60\xEA\x60\xEA\0\x02\x02\x44\x01\0;";
    let file = engine::read("giant.gif".into(), gif.to_vec(), &Options::default());
    let code = file.error.map(|e| e.code);
    assert!(
        matches!(code, Some("image_too_large" | "corrupt_image")),
        "{code:?}"
    );
}

#[test]
fn an_unknown_eci_payload_keeps_an_escaped_text() {
    let payload = b"\0\xFFQA\x1B[31m";
    let qr = zxingcpp::create(QRCode)
        .options(r#"{"eci":"999999"}"#)
        .from_slice(payload)
        .unwrap();
    let found = read(&png(&common::image(qr)), &Options::default());
    let c = &found.codes[0];
    assert_eq!(c.content, "unknown_eci");
    let raw = BASE64_STANDARD.decode(c.bytes_base64.as_ref().unwrap());
    assert_eq!(raw.unwrap(), payload);
    assert!(
        c.text.starts_with("<NUL>") && c.text.contains("QA<ESC>[31m"),
        "{:?}",
        c.text
    );
}

#[test]
fn a_tiff_with_no_readable_page_is_an_error() {
    let tiff = packed_tiff(&GrayImage::new(1, 1), 1, |v| v >> 7);
    // Directory entries are 12 bytes from offset 2, each with its value at 8.
    let ifd = u32::from_le_bytes(tiff[4..8].try_into().unwrap()) as usize;
    let with = |entry: usize, value: u32| {
        let mut tiff = tiff.clone();
        let at = ifd + 2 + 12 * entry + 8;
        tiff[at..at + 4].copy_from_slice(&value.to_le_bytes());
        tiff
    };
    // Too wide; then a strip that starts past the end of the file.
    let (wide, cut) = (with(0, 70_000), with(5, 1 << 20));
    let wide = engine::read("wide.tif".into(), wide, &Options::default()).error;
    assert_eq!(wide.map(|e| e.code), Some("image_too_large"));
    let cut = engine::read("cut.tif".into(), cut, &Options::default()).error;
    let cut = cut.map(|e| (e.code, e.message));
    assert!(
        cut.as_ref()
            .is_some_and(|(code, message)| *code == "corrupt_image"
                && message.starts_with("Can't decode this TIFF file: ")),
        "{cut:?}"
    );
}

/// Reading `tiff` gives `text`, or a plain "isn't supported" error: never no code, a code the
/// pixels don't hold, or the decoder's own words.
fn reads_or_refuses(tiff: Vec<u8>, text: &str) {
    let (error, texts) = tiff_texts(tiff);
    let refused = |e: &String| e.contains(" supported") && !e.contains("unsupported error");
    assert!(
        error.is_none() && texts == [text]
            || texts.is_empty() && error.as_ref().is_some_and(refused),
        "{error:?} {texts:?}"
    );
}

#[test]
#[ignore = "known bug: R3-01"]
fn a_palette_tiff_reads() {
    // Index 0 is white and 1 black, so reading the indices as grey would invert the code.
    let qr = code(QRCode, "palette", "");
    let (w, h) = qr.dimensions();
    let colours = [[65535; 3], [0; 3], [65535, 0, 0], [0, 0, 65535]];
    // ColorMap: every red, then every green, then every blue; unlisted entries are black.
    let map = |bits: u8| -> Vec<u32> {
        let entry = move |c: usize| (0..1 << bits).map(move |i| colours.get(i).map_or(0, |v| v[c]));
        (0..3).flat_map(entry).collect()
    };
    for bits in [1u8, 2, 4, 8] {
        let strip = pack(&qr, bits, |v| u8::from(v < 128));
        let tags = [(320, 3, map(bits))];
        let tiff = raw_tiff((w, h), bits.into(), 1, 3, &[], &[strip], &tags);
        assert_eq!(
            tiff_texts(tiff),
            (Option::None, vec!["palette".into()]),
            "{bits}-bit"
        );
    }
    // White, black, red and blue, through Rec. 601.
    let tiff = raw_tiff(
        (4, 1),
        8,
        1,
        3,
        &[],
        &[vec![0, 1, 2, 3]],
        &[(320, 3, map(8))],
    );
    assert_eq!(tiff_grey(&tiff), Ok(vec![255, 0, 76, 29]));
}

#[test]
#[ignore = "known bug: R3-02"]
fn a_tiff_filled_from_the_low_bit_reads_or_is_refused() {
    // FillOrder 2: each byte's bits run from the lowest, as a fax modem sends them.
    let qr = code(QRCode, "fill order", "");
    let (w, h) = qr.dimensions();
    for bits in [1u8, 4] {
        let strip = pack(&qr, bits, |v| v >> (8 - bits))
            .iter()
            .map(|b| b.reverse_bits())
            .collect();
        let tiff = raw_tiff(
            (w, h),
            bits.into(),
            1,
            1,
            &[],
            &[strip],
            &[(266, 3, vec![2])],
        );
        reads_or_refuses(tiff, "fill order");
    }
}

#[test]
#[ignore = "known bug: R3-04"]
fn a_white_is_zero_grey_alpha_tiff_reads_or_is_refused() {
    let qr = code(QRCode, "white is zero", "");
    let (w, h) = qr.dimensions();
    let (ink, opaque) = (
        qr.iter().map(|v| 255 - v).collect::<Vec<_>>(),
        vec![255; qr.len()],
    );
    for bits in [8, 16] {
        let strip = chunky(&[&ink, &opaque], bits);
        reads_or_refuses(
            raw_tiff((w, h), bits, 2, 0, &[2], &[strip], &[]),
            "white is zero",
        );
    }
}

#[test]
#[ignore = "known bug: R3-05"]
fn premultiplied_cmyk_is_composited_once() {
    // Premultiplied black ink at full, half and no coverage: over white (no ink) it stays as
    // stored, so half-covered black is mid grey.
    let (none, k, alpha) = ([0; 3], [255, 128, 0], [255, 128, 0]);
    let cmyka = chunky(&[&none, &none, &none, &k, &alpha], 8);
    let tiff = raw_tiff((3, 1), 8, 5, 5, &[1], &[cmyka], &[]);
    assert_eq!(tiff_grey(&tiff), Ok(vec![0, 127, 255]));
}

#[test]
#[ignore = "known bug: R3-06"]
fn alpha_after_an_unspecified_extra_sample_is_composited() {
    // ExtraSamples [0, 2]: the second extra sample is the alpha.
    let (grey, other, alpha) = ([0; 3], [255, 0, 77], [0, 255, 128]);
    let pixels = chunky(&[&grey, &other, &alpha], 8);
    let tiff = raw_tiff((3, 1), 8, 3, 1, &[0, 2], &[pixels], &[]);
    assert_eq!(tiff_grey(&tiff), Ok(vec![255, 0, 127]));
}
