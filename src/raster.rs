//! Single images to 8-bit grey: PNG, GIF, BMP, WebP and PNM through `image`.

use std::io::Cursor;

use image::{
    DynamicImage, GrayImage, ImageBuffer, ImageDecoder, ImageFormat, ImageReader, Limits, Pixel,
};

use crate::{error::Error, input::Kind};

const MAX_SIDE: u32 = 65_535;
const MAX_PIXELS: u64 = 120_000_000;

/// Decodes a one-image file to grey, refusing oversized images from the header alone.
pub fn grey(kind: Kind, bytes: &[u8]) -> Result<GrayImage, Error> {
    let format = match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Gif => ImageFormat::Gif,
        Kind::Bmp => ImageFormat::Bmp,
        Kind::Webp => ImageFormat::WebP,
        Kind::Pnm => ImageFormat::Pnm,
        Kind::Jpeg | Kind::Tiff | Kind::Pdf => unreachable!("{} isn't read here", kind.name()),
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
    Ok(to_grey(DynamicImage::from_decoder(decoder).map_err(fail)?))
}

/// `image_too_large` above 65,535 pixels per side or 120 megapixels.
pub fn check_size(width: u64, height: u64) -> Result<(), Error> {
    if width.max(height) > MAX_SIDE.into() || width * height > MAX_PIXELS {
        return Err(Error::image_too_large(width, height));
    }
    Ok(())
}

fn to_grey(img: DynamicImage) -> GrayImage {
    match img {
        DynamicImage::ImageLuma8(grey) => grey,
        DynamicImage::ImageLumaA8(img) => luma(&img),
        DynamicImage::ImageRgb8(img) => luma(&img),
        DynamicImage::ImageRgba8(img) => luma(&img),
        img @ DynamicImage::ImageLuma16(_) => img.into_luma8(),
        img => luma(&img.into_rgba8()),
    }
}

/// Rec. 601 luma as zxing-cpp computes it, with any alpha composited over white.
pub fn luma<P: Pixel<Subpixel = u8>>(img: &ImageBuffer<P, Vec<u8>>) -> GrayImage {
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

    fn encode(img: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
        let mut bytes = Cursor::new(vec![]);
        img.into().write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn composites_alpha_over_white() {
        let img = ImageBuffer::from_vec(3, 1, vec![0, 255, 0, 0, 0, 128]).unwrap();
        let img: ImageBuffer<LumaA<u8>, _> = img;
        let out = grey(Kind::Png, &encode(img, ImageFormat::Png)).unwrap();
        assert_eq!(out.as_raw(), &[0, 255, 127]);
    }

    #[test]
    fn weighs_colours_as_rec_601() {
        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_vec(3, 1, vec![255, 0, 0, 0, 255, 0, 0, 0, 255]).unwrap();
        let out = grey(Kind::Bmp, &encode(img, ImageFormat::Bmp)).unwrap();
        assert_eq!(out.as_raw(), &[76, 150, 29]);
    }

    #[test]
    fn scales_16_bit_samples_to_8() {
        let img: ImageBuffer<Luma<u16>, _> = ImageBuffer::from_vec(2, 1, vec![0, 65535]).unwrap();
        let out = grey(Kind::Png, &encode(img, ImageFormat::Png)).unwrap();
        assert_eq!(out.as_raw(), &[0, 255]);
        let img: ImageBuffer<Rgba<u16>, _> =
            ImageBuffer::from_vec(1, 1, vec![65535, 65535, 65535, 0]).unwrap();
        assert_eq!(
            grey(Kind::Png, &encode(img, ImageFormat::Png))
                .unwrap()
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
        let e = grey(Kind::Bmp, &with_size(70_000, 2)).unwrap_err();
        assert_eq!(e.code, "image_too_large");
        assert_eq!(
            e.message,
            "The image is 70000x2 pixels; the limit is 120 megapixels and 65,535 pixels per side."
        );
        assert_eq!(e.hint, Some("Crop or resize it first."));
        let e = grey(Kind::Bmp, &with_size(11_000, 11_000)).unwrap_err();
        assert_eq!(e.code, "image_too_large");
        let wide = encode(GrayImage::new(65_536, 1), ImageFormat::Png);
        assert_eq!(grey(Kind::Png, &wide).unwrap_err().code, "image_too_large");
        assert!(check_size(65_535, 1_831).is_ok());
    }

    #[test]
    fn reports_the_decoder_message_for_a_damaged_file() {
        let png = encode(GrayImage::new(64, 64), ImageFormat::Png);
        let e = grey(Kind::Png, &png[..png.len() / 2]).unwrap_err();
        assert_eq!(e.code, "corrupt_image");
        assert!(
            e.message.starts_with("Can't decode this PNG file: "),
            "{}",
            e.message
        );
        assert!(e.message.ends_with('.') && !e.message.ends_with(".."));
        assert_eq!(e.hint, Some("The file may be damaged or cut short."));
    }
}
