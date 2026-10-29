//! Single images to 8-bit grey: JPEG with zune-jpeg; PNG, GIF, BMP, WebP and PNM with `image`.

use std::io::Cursor;

use image::{
    DynamicImage, GrayImage, ImageBuffer, ImageDecoder, ImageFormat, ImageReader, Limits, Pixel,
    RgbImage, metadata::Orientation,
};

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};

use crate::decode::Page;
use crate::error::{Error, Warning};
use crate::input::Kind;

const MAX_SIDE: u32 = 65_535;
const MAX_PIXELS: u64 = 120_000_000;

/// Decodes a one-image file to grey with its EXIF orientation, refusing oversized images from
/// the header alone. Animations are read from their first frame, with a warning.
pub fn image(kind: Kind, bytes: &[u8], warnings: &mut Vec<Warning>) -> Result<Page, Error> {
    let format = match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Gif => ImageFormat::Gif,
        Kind::Bmp => ImageFormat::Bmp,
        Kind::Webp => ImageFormat::WebP,
        Kind::Pnm => ImageFormat::Pnm,
        Kind::Jpeg => return jpeg(bytes),
        Kind::Tiff | Kind::Pdf => unreachable!("{} isn't read here", kind.name()),
    };
    let fail = |e| Error::corrupt_image(kind.label(), e);
    // Our own size check runs first, so an oversized image is reported with its size. The BMP
    // decoder refuses sides over 65,535 while reading its header, so BMP sizes are read here.
    if let (Kind::Bmp, Some(info)) = (kind, bytes.get(14..26))
        && info[..4] != [12, 0, 0, 0]
    {
        let side = |at: usize| i32::from_le_bytes(info[at..at + 4].try_into().unwrap());
        check_size(side(4).unsigned_abs().into(), side(8).unsigned_abs().into())?;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(fail)?;
    let (width, height) = decoder.dimensions();
    check_size(width.into(), height.into())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(1 << 30);
    decoder.set_limits(limits).map_err(fail)?;
    let orientation = decoder.orientation().map_err(fail)?;
    let grey = to_grey(DynamicImage::from_decoder(decoder).map_err(fail)?);
    if animated(kind, bytes) {
        warnings.push(Warning::first_frame_only());
    }
    Ok(Page {
        orientation,
        ..Page::new(1, grey)
    })
}

/// A WebP says so in its VP8X flags; a GIF has a second frame, found without decoding any.
fn animated(kind: Kind, bytes: &[u8]) -> bool {
    match kind {
        Kind::Webp => {
            bytes.get(12..16) == Some(b"VP8X") && bytes.get(20).is_some_and(|f| f & 2 != 0)
        }
        Kind::Gif => {
            let mut options = gif::DecodeOptions::new();
            options.skip_frame_decoding(true);
            options
                .read_info(bytes)
                .is_ok_and(|mut gif| (0..2).all(|_| matches!(gif.next_frame_info(), Ok(Some(_)))))
        }
        _ => false,
    }
}

/// `image_too_large` above 65,535 pixels per side or 120 megapixels.
pub fn check_size(width: u64, height: u64) -> Result<(), Error> {
    if width.max(height) > MAX_SIDE.into() || width * height > MAX_PIXELS {
        return Err(Error::image_too_large(width, height));
    }
    Ok(())
}

pub(crate) fn to_grey(img: DynamicImage) -> GrayImage {
    match img {
        DynamicImage::ImageLuma8(grey) => grey,
        DynamicImage::ImageLumaA8(img) => luma(&img),
        DynamicImage::ImageRgb8(img) => luma(&img),
        DynamicImage::ImageRgba8(img) => luma(&img),
        img @ DynamicImage::ImageLuma16(_) => img.into_luma8(),
        img => luma(&img.into_rgba8()),
    }
}

/// JPEG straight to grey with zune-jpeg. A file the strict decoder refuses (cut short, damaged
/// data, stray bytes) is decoded again leniently, as browsers show it, and marked damaged.
fn jpeg(bytes: &[u8]) -> Result<Page, Error> {
    let options = DecoderOptions::default()
        .set_max_width(65_535)
        .set_max_height(65_535);
    match decode_jpeg(bytes, options.set_strict_mode(true)) {
        Err(e) if e.code == "corrupt_image" => Ok(Page {
            damaged: Some(e),
            ..decode_jpeg(bytes, options)?
        }),
        read => read,
    }
}

/// CMYK, YCCK and RGB JPEGs go through RGB first.
fn decode_jpeg(bytes: &[u8], options: DecoderOptions) -> Result<Page, Error> {
    let fail = |e| Error::corrupt_image("JPEG", e);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(fail)?;
    let (width, height) = decoder.dimensions().expect("headers are decoded");
    check_size(width as u64, height as u64)?;
    let (width, height) = (width as u32, height as u32);
    let orientation = decoder
        .exif()
        .and_then(|exif| Orientation::from_exif_chunk(exif));
    let grey = if matches!(
        decoder.input_colorspace(),
        Some(ColorSpace::YCbCr | ColorSpace::Luma)
    ) {
        decoder.set_options(options.jpeg_set_out_colorspace(ColorSpace::Luma));
        let pixels = decoder.decode().map_err(fail)?;
        GrayImage::from_vec(width, height, pixels).expect("one byte per pixel")
    } else {
        let rgb = RgbImage::from_vec(width, height, decoder.decode().map_err(fail)?);
        luma(&rgb.expect("three bytes per pixel"))
    };
    Ok(Page {
        orientation: orientation.unwrap_or(Orientation::NoTransforms),
        ..Page::new(1, grey)
    })
}

/// Rec. 601 luma as zxing-cpp computes it, with any alpha composited over white.
pub fn luma<P, C>(img: &ImageBuffer<P, C>) -> GrayImage
where
    P: Pixel<Subpixel = u8>,
    C: std::ops::Deref<Target = [u8]>,
{
    let pixels = img.pixels().map(|p| {
        let [r, g, b, a] = p.to_rgba().0.map(u32::from);
        let l = (306 * r + 601 * g + 117 * b + 512) >> 10;
        (l * a / 255 + 255 - a) as u8
    });
    GrayImage::from_vec(img.width(), img.height(), pixels.collect()).expect("one byte per pixel")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Luma, LumaA, Rgb, Rgba};

    fn image_of(kind: Kind, bytes: &[u8]) -> Result<Page, Error> {
        image(kind, bytes, &mut vec![])
    }

    fn encode(img: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(vec![]);
        img.into().write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn composites_alpha_over_white() {
        let img = ImageBuffer::from_vec(3, 1, vec![0, 255, 0, 0, 0, 128]).unwrap();
        let img: ImageBuffer<LumaA<u8>, _> = img;
        let out = image_of(Kind::Png, &encode(img, ImageFormat::Png))
            .unwrap()
            .grey;
        assert_eq!(out.as_raw(), &[0, 255, 127]);
    }

    #[test]
    fn weighs_colours_as_rec_601() {
        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_vec(3, 1, vec![255, 0, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let out = image_of(Kind::Bmp, &encode(img, ImageFormat::Bmp))
            .unwrap()
            .grey;
        assert_eq!(out.as_raw(), &[76, 150, 29]);
    }

    #[test]
    fn scales_16_bit_samples_to_8() {
        let img: ImageBuffer<Luma<u16>, _> = ImageBuffer::from_vec(2, 1, vec![0, 65535]).unwrap();
        let out = image_of(Kind::Png, &encode(img, ImageFormat::Png))
            .unwrap()
            .grey;
        assert_eq!(out.as_raw(), &[0, 255]);
        let img: ImageBuffer<Rgba<u16>, _> =
            ImageBuffer::from_vec(1, 1, vec![65535, 65535, 65535, 0]).unwrap();
        assert_eq!(
            image_of(Kind::Png, &encode(img, ImageFormat::Png))
                .unwrap()
                .grey
                .as_raw(),
            &[255]
        );
    }

    #[test]
    fn refuses_oversized_images_from_the_header() {
        let bmp = encode(GrayImage::new(2, 2), ImageFormat::Bmp);
        let with_size = |w: i32, h: i32| {
            let mut b = bmp.clone();
            b[18..22].copy_from_slice(&w.to_le_bytes());
            b[22..26].copy_from_slice(&h.to_le_bytes());
            b
        };
        let e = image_of(Kind::Bmp, &with_size(70_000, 2)).unwrap_err();
        assert_eq!(e.code, "image_too_large");
        assert_eq!(
            e.message,
            "The image is 70000x2 pixels; the limit is 120 megapixels and 65,535 pixels per side."
        );
        assert_eq!(e.hint, Some("Crop or resize it first."));
        let e = image_of(Kind::Bmp, &with_size(11_000, 11_000)).unwrap_err();
        assert_eq!(e.code, "image_too_large");
        let wide = encode(GrayImage::new(65_536, 1), ImageFormat::Png);
        assert_eq!(
            image_of(Kind::Png, &wide).unwrap_err().code,
            "image_too_large"
        );
        assert!(check_size(65_535, 1_831).is_ok());
    }

    #[test]
    fn reports_the_decoder_message_for_a_damaged_file() {
        let png = encode(GrayImage::new(64, 64), ImageFormat::Png);
        let e = image_of(Kind::Png, &png[..png.len() / 2]).unwrap_err();
        assert_eq!(e.code, "corrupt_image");
        assert!(
            e.message.starts_with("Can't decode this PNG file: "),
            "{}",
            e.message
        );
        assert!(e.message.ends_with('.') && !e.message.ends_with(".."));
        assert_eq!(e.hint, Some("The file may be damaged or cut short."));
    }

    /// The segments before the scan, then the scan header with its data but not the end marker.
    fn segments(jpeg: &[u8]) -> Vec<&[u8]> {
        let (mut out, mut at) = (vec![], 2);
        while jpeg[at + 1] != 0xDA {
            let end = at + 2 + usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
            out.push(&jpeg[at..end]);
            at = end;
        }
        out.push(&jpeg[at..jpeg.len() - 2]);
        out
    }

    /// An Adobe CMYK JPEG (inverted samples, as Photoshop writes them) whose black plane is
    /// `img`: the scans of four greyscale JPEGs under one four-component frame.
    fn cmyk_jpeg(img: &GrayImage) -> Vec<u8> {
        let paper = GrayImage::from_pixel(img.width(), img.height(), Luma([255]));
        let planes = [&paper, &paper, &paper, img].map(|p| encode(p.clone(), ImageFormat::Jpeg));
        let mut out = b"\xFF\xD8\xFF\xEE\0\x0EAdobe\0\x64\0\0\0\0\0".to_vec();
        for table in segments(&planes[0])
            .iter()
            .filter(|s| matches!(s[1], 0xDB | 0xC4))
        {
            out.extend_from_slice(table);
        }
        let (w, h) = (img.width() as u16, img.height() as u16);
        out.extend(
            [
                [0xFF, 0xC0, 0, 20, 8].as_slice(),
                &h.to_be_bytes(),
                &w.to_be_bytes(),
                &[4],
            ]
            .concat(),
        );
        for id in 1..=4 {
            out.extend([id, 0x11, 0]);
        }
        for (id, plane) in (1..=4).zip(&planes) {
            let mut scan = segments(plane).pop().unwrap().to_vec();
            scan[5] = id;
            out.extend(scan);
        }
        out.extend([0xFF, 0xD9]);
        out
    }

    #[test]
    fn reads_grey_colour_and_cmyk_jpegs() {
        let img = GrayImage::from_fn(64, 32, |x, _| Luma([if x < 32 { 0 } else { 255 }]));
        let rgb = DynamicImage::ImageLuma8(img.clone()).into_rgb8();
        let jpegs = [
            encode(img.clone(), ImageFormat::Jpeg),
            encode(rgb, ImageFormat::Jpeg),
            cmyk_jpeg(&img),
        ];
        for jpeg in jpegs {
            let out = image_of(Kind::Jpeg, &jpeg).unwrap().grey;
            assert_eq!(out.dimensions(), (64, 32));
            assert!(
                out[(4, 16)].0[0] < 30 && out[(60, 16)].0[0] > 225,
                "{:?}",
                out[(60, 16)]
            );
        }
    }

    #[test]
    fn refuses_oversized_or_broken_jpegs() {
        let mut jpeg = encode(GrayImage::new(8, 8), ImageFormat::Jpeg);
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        jpeg[sof + 5..sof + 9].copy_from_slice(&[0x4E, 0x20, 0x4E, 0x20]);
        assert_eq!(
            image_of(Kind::Jpeg, &jpeg).unwrap_err().message,
            "The image is 20000x20000 pixels; the limit is 120 megapixels and 65,535 pixels per side."
        );
        let e = image_of(Kind::Jpeg, b"\xFF\xD8\xFF\xC0\0\x02").unwrap_err();
        assert!(
            e.message.starts_with("Can't decode this JPEG file: "),
            "{}",
            e.message
        );
    }
}
