//! TIFF files page by page with the tiff crate, fax pages included.

use std::io::Cursor;

use ::tiff::decoder::{Decoder, DecodingResult, Limits};
use ::tiff::tags::{CompressionMethod, Tag};
use ::tiff::{ColorType, TiffError, TiffUnsupportedError};
use image::{DynamicImage, GrayImage, ImageBuffer, RgbImage, metadata::Orientation};

use crate::Options;
use crate::decode::Page;
use crate::error::{Error, Warning};
use crate::input::select;
use crate::raster::{check_size, to_grey};

type Tiff<'a> = Decoder<Cursor<&'a [u8]>>;

/// Reads the selected pages (`options.pages`) in order and hands each to `each`. A page that
/// can't be decoded becomes a `page_failed` warning and the others still read. An unreadable
/// first directory fails the file, a broken later one ends it, and so does the first page's
/// error when no selected page could be read.
pub fn pages(
    bytes: &[u8],
    options: &Options,
    warnings: &mut Vec<Warning>,
    mut each: impl FnMut(Page),
) -> Result<(), Error> {
    // Room for the largest image allowed (120 megapixels of 16-bit RGBA).
    let mut limits = Limits::default();
    limits.decoding_buffer_size = 1 << 30;
    let mut tiff = Decoder::new(Cursor::new(bytes))
        .map_err(|e| Error::corrupt_image("TIFF", e))?
        .with_limits(limits);
    let mut count = 1;
    while tiff.more_images() {
        if let Err(e) = tiff.next_image() {
            warnings.push(Warning::page_failed(count + 1, reason(e)));
            break;
        }
        count += 1;
    }
    let (mut read, mut failed) = (false, None);
    for number in select(options.pages.as_ref(), count, warnings)? {
        let page = tiff.seek_to_image(number - 1).map_err(unreadable);
        match page.and_then(|()| self::page(&mut tiff, number)) {
            Ok(page) => {
                read = true;
                each(page);
            }
            Err(e) => {
                warnings.push(Warning::page_failed(number, &e.message));
                failed.get_or_insert(e);
            }
        }
    }
    match failed {
        Some(e) if !read && e.code == "corrupt_image" => {
            Err(Error::corrupt_image("TIFF", e.message))
        }
        Some(e) if !read => Err(e),
        _ => Ok(()),
    }
}

/// The page, or why not: `image_too_large`, or `corrupt_image` holding the bare reason.
fn page(tiff: &mut Tiff, number: usize) -> Result<Page, Error> {
    let (w, h) = tiff.dimensions().map_err(unreadable)?;
    check_size(w.into(), h.into())?;
    let orientation = tiff.find_tag_unsigned(Tag::Orientation).ok().flatten();
    let color = tiff.colortype().map_err(unreadable)?;
    let pixels = tiff.read_image().map_err(unreadable)?;
    let img = match (color, pixels) {
        (ColorType::Gray(8), DecodingResult::U8(v)) => {
            GrayImage::from_raw(w, h, v).map(DynamicImage::ImageLuma8)
        }
        (ColorType::Gray(16), DecodingResult::U16(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageLuma16)
        }
        (ColorType::GrayA(8), DecodingResult::U8(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageLumaA8)
        }
        (ColorType::GrayA(16), DecodingResult::U16(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageLumaA16)
        }
        (ColorType::RGB(8), DecodingResult::U8(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageRgb8)
        }
        (ColorType::RGB(16), DecodingResult::U16(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageRgb16)
        }
        (ColorType::RGBA(8), DecodingResult::U8(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageRgba8)
        }
        (ColorType::RGBA(16), DecodingResult::U16(v)) => {
            ImageBuffer::from_raw(w, h, v).map(DynamicImage::ImageRgba16)
        }
        // JPEG-compressed pages come back as full-size YCbCr, whose Y is the grey we want.
        (ColorType::YCbCr(8), DecodingResult::U8(v)) if v.len() == w as usize * h as usize * 3 => {
            GrayImage::from_raw(w, h, v.into_iter().step_by(3).collect())
                .map(DynamicImage::ImageLuma8)
        }
        (ColorType::CMYK(8), DecodingResult::U8(v)) => {
            let rgb = v.as_chunks::<4>().0.iter().flat_map(|&[c, m, y, k]| {
                let k = 255 - u32::from(k);
                [c, m, y].map(|v| ((255 - u32::from(v)) * k / 255) as u8)
            });
            RgbImage::from_raw(w, h, rgb.collect()).map(DynamicImage::ImageRgb8)
        }
        // Bilevel and other packed grey: the tiff crate has already turned WhiteIsZero around.
        (ColorType::Gray(bits @ (1 | 2 | 4)), DecodingResult::U8(v)) => {
            // 255 is a multiple of 1, 3 and 15, so each level maps exactly and never overflows.
            let step = 255 / ((1 << bits) - 1);
            let samples = unpack(&v, bits, w, h).map(|s| s.into_iter().map(|s| s * step).collect());
            samples
                .and_then(|s| GrayImage::from_raw(w, h, s))
                .map(DynamicImage::ImageLuma8)
        }
        (ColorType::Palette(bits @ (1 | 2 | 4 | 8)), DecodingResult::U8(v)) => {
            let map = tiff.get_tag_u16_vec(Tag::ColorMap).map_err(unreadable)?;
            let size = 1 << bits;
            let colour = |i: u8, c: usize| {
                map.get(c * size + usize::from(i))
                    .map_or(0, |v| (v >> 8) as u8)
            };
            let rgb = unpack(&v, bits, w, h).map(|s| {
                s.into_iter()
                    .flat_map(|i| [0, 1, 2].map(|c| colour(i, c)))
                    .collect()
            });
            rgb.and_then(|rgb| RgbImage::from_raw(w, h, rgb))
                .map(DynamicImage::ImageRgb8)
        }
        (color, _) => return Err(bad(format!("{color:?} pixels aren't supported"))),
    };
    let grey = to_grey(img.ok_or_else(|| bad("its pixel data is cut short".into()))?);
    Ok(Page {
        orientation: orientation
            .and_then(Orientation::from_exif)
            .unwrap_or(Orientation::NoTransforms),
        ..Page::new(number, grey)
    })
}

/// Samples of `bits` bits each, packed from the high bit, every row starting on a new byte.
fn unpack(data: &[u8], bits: u8, w: u32, h: u32) -> Option<Vec<u8>> {
    let (bits, w) = (usize::from(bits), w as usize);
    let row = (w * bits).div_ceil(8);
    let mask = (1u16 << bits) as u8 - 1;
    let rows = data.get(..row * h as usize)?.chunks_exact(row);
    Some(
        rows.flat_map(|r| (0..w).map(move |x| r[x * bits / 8] >> (8 - bits - x * bits % 8) & mask))
            .collect(),
    )
}

fn bad(reason: String) -> Error {
    Error::new("corrupt_image", reason, None)
}

fn unreadable(e: TiffError) -> Error {
    bad(reason(e))
}

fn reason(e: TiffError) -> String {
    match e {
        TiffError::UnsupportedError(TiffUnsupportedError::UnsupportedCompressionMethod(
            CompressionMethod::Fax3,
        )) => "Group 3 fax compression isn't supported".into(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::tiff::encoder::{TiffEncoder, colortype};

    #[test]
    fn unpacks_rows_that_start_on_a_byte() {
        // Two rows of three 1-bit samples, each row padded to a byte.
        assert_eq!(
            unpack(&[0b1010_0000, 0b0110_0000], 1, 3, 2),
            Some(vec![1, 0, 1, 0, 1, 1])
        );
        assert_eq!(unpack(&[0x1F, 0xA0], 4, 3, 1), Some(vec![1, 15, 10]));
        assert_eq!(unpack(&[0xFF], 1, 3, 2), None);
    }

    #[test]
    fn reads_every_page_with_its_number_size_and_orientation() {
        let mut bytes = Cursor::new(vec![]);
        let mut tiff = TiffEncoder::new(&mut bytes).unwrap();
        tiff.write_image::<colortype::Gray8>(3, 1, &[0, 128, 255])
            .unwrap();
        let mut page = tiff.new_image::<colortype::RGB8>(2, 1).unwrap();
        page.encoder().write_tag(Tag::Orientation, 6u16).unwrap();
        page.write_data(&[255, 0, 0, 0, 0, 255]).unwrap();
        tiff.write_image::<colortype::Gray16>(1, 1, &[65535])
            .unwrap();
        tiff.write_image::<colortype::CMYK8>(1, 1, &[0, 0, 0, 255])
            .unwrap();

        let mut read = vec![];
        let mut warnings = vec![];
        let options = Options::default();
        pages(bytes.get_ref(), &options, &mut warnings, |p| read.push(p)).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let summary: Vec<_> = read
            .iter()
            .map(|p| (p.number, p.size(), p.grey.as_raw().clone()))
            .collect();
        assert_eq!(
            summary,
            [
                (1, (3, 1), vec![0, 128, 255]),
                (2, (1, 2), vec![76, 29]),
                (3, (1, 1), vec![255]),
                (4, (1, 1), vec![0])
            ]
        );
        assert_eq!(read[1].orientation, Orientation::Rotate90);

        let options = Options {
            pages: Some(crate::input::PageList::parse("2,4-").unwrap()),
            ..Options::default()
        };
        let mut numbers = vec![];
        pages(bytes.get_ref(), &options, &mut warnings, |p| {
            numbers.push(p.number)
        })
        .unwrap();
        assert_eq!((numbers, warnings), (vec![2, 4], vec![]));
    }

    #[test]
    fn a_damaged_file_is_corrupt_image() {
        let bytes = b"II*\0\xFF\xFF\xFF\x7F";
        let e = pages(bytes, &Options::default(), &mut vec![], |_| ()).unwrap_err();
        assert_eq!(e.code, "corrupt_image");
        assert!(
            e.message.starts_with("Can't decode this TIFF file: "),
            "{}",
            e.message
        );
    }
}
