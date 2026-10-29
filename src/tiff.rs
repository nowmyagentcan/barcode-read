//! TIFF files page by page with the tiff crate, fax pages included.

use std::io::Cursor;

use ::tiff::decoder::{BufferLayoutPreference, ChunkType, Decoder, DecodingResult, Limits};
use ::tiff::tags::{ByteOrder, CompressionMethod, SampleFormat, Tag};
use ::tiff::{ColorType, TiffError, TiffUnsupportedError};
use image::{GrayImage, metadata::Orientation};

use crate::Options;
use crate::decode::Page;
use crate::error::{Error, Warning};
use crate::input::select;
use crate::raster::{check_size, over_white, rec601};

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
    let mut tiff = open(bytes)?;
    // Pages the tiff crate can't read as stored are mended in one copy of the file.
    let (mut copy, mut palette) = (None, vec![]);
    loop {
        palette.push(mend(&mut tiff, bytes, &mut copy));
        if !tiff.more_images() {
            break;
        }
        if let Err(e) = tiff.next_image() {
            warnings.push(Warning::page_failed(palette.len() + 1, reason(e)));
            break;
        }
    }
    let count = palette.len();
    if let Some(copy) = &copy {
        tiff = open(copy)?;
    }
    let (mut read, mut failed) = (false, None);
    for number in select(options.pages.as_ref(), count, warnings)? {
        let page = tiff.seek_to_image(number - 1).map_err(unreadable);
        match page.and_then(|()| self::page(&mut tiff, number, palette[number - 1])) {
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

fn open(bytes: &[u8]) -> Result<Tiff<'_>, Error> {
    let tiff = Decoder::new(Cursor::new(bytes)).map_err(|e| Error::corrupt_image("TIFF", e))?;
    Ok(tiff.with_limits(limits()))
}

/// Rewrites the current page in `copy` (made from `bytes` when first needed) where tiff 0.11.3
/// can't read it as stored, and returns whether it is a palette page. Palette pages are marked
/// BlackIsZero, as tiff refuses them, so the indices come back as grey samples; the data of
/// pages filled from the low bit of each byte (FillOrder 2, which tiff ignores) is turned
/// around, except JPEG's, as libtiff does.
fn mend(tiff: &mut Tiff, bytes: &[u8], copy: &mut Option<Vec<u8>>) -> bool {
    let mut tag = |tag| tiff.find_tag_unsigned::<u16>(tag).ok().flatten();
    let palette = tag(Tag::PhotometricInterpretation) == Some(3);
    let reversed = tag(Tag::FillOrder) == Some(2) && !matches!(tag(Tag::Compression), Some(6 | 7));
    if palette && let Some(at) = entry(tiff, bytes, Tag::PhotometricInterpretation) {
        let one = match tiff.byte_order() {
            ByteOrder::LittleEndian => 1u16.to_le_bytes(),
            ByteOrder::BigEndian => 1u16.to_be_bytes(),
        };
        copy.get_or_insert_with(|| bytes.to_vec())[at..at + 2].copy_from_slice(&one);
    }
    if reversed {
        let (offsets, counts) = match tiff.get_chunk_type() {
            ChunkType::Strip => (Tag::StripOffsets, Tag::StripByteCounts),
            ChunkType::Tile => (Tag::TileOffsets, Tag::TileByteCounts),
        };
        let (offsets, counts) = (tiff.get_tag_u64_vec(offsets), tiff.get_tag_u64_vec(counts));
        let copy = copy.get_or_insert_with(|| bytes.to_vec());
        let chunks = offsets.unwrap_or_default().into_iter();
        for (at, n) in chunks.zip(counts.unwrap_or_default()) {
            let (at, n) = (at as usize, n as usize);
            if let Some(chunk) = copy.get_mut(at..at.saturating_add(n)) {
                chunk.iter_mut().for_each(|b| *b = b.reverse_bits());
            }
        }
    }
    palette
}

/// Where the current page's `tag`, a SHORT, has its value in `bytes`.
fn entry(tiff: &mut Tiff, bytes: &[u8], tag: Tag) -> Option<usize> {
    let order = tiff.byte_order();
    let short = |at: usize| {
        let b = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(match order {
            ByteOrder::LittleEndian => u16::from_le_bytes(b),
            ByteOrder::BigEndian => u16::from_be_bytes(b),
        })
    };
    // Only classic TIFF is read (input.rs): 12-byte entries after a 2-byte count.
    let ifd = usize::try_from(tiff.ifd_pointer()?.0).ok()?;
    let at = (0..short(ifd)?.into())
        .map(|i: usize| ifd + 2 + i * 12)
        .find(|&at| short(at) == Some(tag.to_u16()) && short(at + 2) == Some(3))?;
    short(at + 8).map(|_| at + 8)
}

/// The page, or why not: `image_too_large`, or `corrupt_image` holding the bare reason.
fn page(tiff: &mut Tiff, number: usize, palette: bool) -> Result<Page, Error> {
    let (w, h) = tiff.dimensions().map_err(unreadable)?;
    check_size(w.into(), h.into())?;
    let orientation = tiff.find_tag_unsigned(Tag::Orientation).ok().flatten();
    let color = match tiff.colortype().map_err(unreadable)? {
        ColorType::Gray(bits) if palette => ColorType::Palette(bits),
        color => color,
    };
    let layout = tiff.image_buffer_layout().map_err(unreadable)?;
    let planar = layout.planes > 1 && color.bit_depth() % 8 == 0;
    let bits = u32::from(color.bit_depth())
        * if planar {
            1
        } else {
            u32::from(color.num_samples())
        };
    let pixels = match planar || tiff.get_chunk_type() == ChunkType::Tile {
        true => chunks(tiff, &layout, bits, w, h)?,
        false => tiff.read_image().map_err(unreadable)?,
    };
    let colours = match color {
        ColorType::Gray(_) | ColorType::Multiband { .. } => 1,
        ColorType::RGB(_) | ColorType::RGBA(_) => 3,
        ColorType::CMYK(_) | ColorType::CMYKA(_) => 4,
        _ => 0,
    };
    let img = match (color, pixels) {
        // JPEG-compressed pages come back as full-size YCbCr, whose Y is the grey we want.
        (ColorType::YCbCr(8), DecodingResult::U8(v)) if v.len() == w as usize * h as usize * 3 => {
            GrayImage::from_raw(w, h, v.into_iter().step_by(3).collect())
        }
        // Bilevel and other packed grey: the tiff crate has already turned WhiteIsZero around.
        (ColorType::Gray(bits @ (1 | 2 | 4)), DecodingResult::U8(v)) => {
            // 255 is a multiple of 1, 3 and 15, so each level maps exactly and never overflows.
            let step = 255 / ((1 << bits) - 1);
            let samples = unpack(&v, bits, w, h).map(|s| s.into_iter().map(|s| s * step).collect());
            samples.and_then(|s| GrayImage::from_raw(w, h, s))
        }
        (ColorType::Palette(bits @ (1 | 2 | 4 | 8)), DecodingResult::U8(v)) => {
            let map = tiff.get_tag_u16_vec(Tag::ColorMap).map_err(unreadable)?;
            let size = 1 << bits;
            let colour = |i: u8, c: usize| {
                map.get(c * size + usize::from(i))
                    .map_or(0, |v| u32::from(v >> 8))
            };
            let grey = unpack(&v, bits, w, h).map(|s| {
                s.into_iter()
                    .map(|i| rec601(colour(i, 0), colour(i, 1), colour(i, 2)) as u8)
                    .collect()
            });
            grey.and_then(|grey| GrayImage::from_raw(w, h, grey))
        }
        (_, DecodingResult::U8(v)) if colours > 0 && color.bit_depth() == 8 => {
            samples(tiff, color, colours, &v, planar, w, h)
        }
        (_, DecodingResult::U16(v)) if colours > 0 && color.bit_depth() == 16 => {
            // Scaled to 8 bits as `image` does.
            let v: Vec<u8> = v
                .iter()
                .map(|&s| ((u32::from(s) + 128) / 257) as u8)
                .collect();
            samples(tiff, color, colours, &v, planar, w, h)
        }
        (color, _) => return Err(unsupported(color, layout.sample_format)),
    };
    let grey = img.ok_or_else(|| bad("its pixel data is cut short".into()))?;
    Ok(Page {
        orientation: orientation
            .and_then(Orientation::from_exif)
            .unwrap_or(Orientation::NoTransforms),
        ..Page::new(number, grey)
    })
}

/// Room for the largest image allowed (120 megapixels of 16-bit RGBA).
fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.decoding_buffer_size = 1 << 30;
    limits
}

/// The page chunk by chunk (tiles or strips), plane after plane, as rows of `bits`-bit pixels.
/// Each chunk is read at its full width and cropped here: tiff 0.11.3 overruns its buffer on the
/// bottom tiles of planar pages (image-rs/image-tiff#403) and loses LZW data when it crops tiles
/// itself (image-rs/image-tiff#395).
fn chunks(
    tiff: &mut Tiff,
    layout: &BufferLayoutPreference,
    bits: u32,
    w: u32,
    h: u32,
) -> Result<DecodingResult, Error> {
    let mut pixels = DecodingResult::U8(vec![]);
    pixels.resize_to(layout, &limits()).map_err(unreadable)?;
    let mut out = pixels.as_buffer(0);
    let out = out.as_bytes_mut();
    let (tw, th) = tiff.chunk_dimensions();
    let across = w.div_ceil(tw);
    let per_plane = across * h.div_ceil(th);
    let bytes = |pixels: u32| (pixels as usize * bits as usize).div_ceil(8);
    let (row, stride) = (bytes(w), bytes(tw));
    let mut chunk = DecodingResult::U8(vec![]);
    for i in 0..per_plane * layout.planes as u32 {
        tiff.read_chunk_to_buffer(&mut chunk, i, stride)
            .map_err(unreadable)?;
        let (plane, i) = (i / per_plane, i % per_plane);
        let (x, y) = (i % across * tw, i / across * th);
        let width = bytes((w - x).min(tw));
        let data = chunk.as_buffer(0);
        let rows = data
            .as_bytes()
            .chunks(stride)
            .take((h - y).min(th) as usize);
        for (r, src) in rows.enumerate() {
            let at = (plane * h + y) as usize * row + r * row + bytes(x);
            let (dst, src) = (out.get_mut(at..at + width), src.get(..width));
            dst.zip(src)
                .ok_or_else(|| bad("its pixel data is cut short".into()))
                .map(|(dst, src)| dst.copy_from_slice(src))?;
        }
    }
    Ok(pixels)
}

/// Grey from 8-bit samples of 1 (grey), 3 (RGB) or 4 (CMYK) `colours`, interleaved or one plane
/// per sample, with an alpha sample composited over white; `None` when the data is cut short.
fn samples(
    tiff: &mut Tiff,
    color: ColorType,
    colours: usize,
    v: &[u8],
    planar: bool,
    w: u32,
    h: u32,
) -> Option<GrayImage> {
    // tiff keeps only the colour samples and, for RGBA and CMYKA, the first extra one.
    let stride = usize::from(color.num_samples());
    let extra = tiff.find_tag_unsigned_vec::<u16>(Tag::ExtraSamples);
    let extra = extra.ok().flatten().unwrap_or_default();
    // ExtraSamples: 1 is premultiplied alpha, 2 plain alpha, 0 other data; the alpha is the
    // first 1 or 2. Like tiff, an extra sample without the tag is taken as plain alpha.
    let alpha = match color {
        ColorType::Gray(_) | ColorType::RGB(_) | ColorType::CMYK(_) => None,
        _ if extra.is_empty() => Some((colours, false)),
        _ => extra
            .iter()
            .position(|e| matches!(e, 1 | 2))
            .map(|i| (colours + i, extra[i] == 1)),
    };
    let n = w as usize * h as usize;
    if stride < alpha.map_or(colours, |(at, _)| at + 1) || v.len() < n * stride {
        return None;
    }
    let sample = |i: usize, s: usize| {
        u32::from(if planar {
            v[s * n + i]
        } else {
            v[i * stride + s]
        })
    };
    let grey = (0..n).map(|i| {
        let s = |k| sample(i, k);
        let l = match colours {
            1 => s(0),
            3 => rec601(s(0), s(1), s(2)),
            _ => {
                let k = 255 - s(3);
                let [r, g, b] = [0, 1, 2].map(|c| (255 - s(c)) * k / 255);
                rec601(r, g, b)
            }
        };
        match alpha {
            None => l as u8,
            // Premultiplied ink over white (no ink) is already the composite.
            Some((_, true)) if colours == 4 => l as u8,
            // Premultiplied grey is already scaled by its coverage.
            Some((at, true)) => (l + 255 - s(at)).min(255) as u8,
            Some((at, false)) => over_white(l, s(at)),
        }
    });
    GrayImage::from_vec(w, h, grey.collect())
}

fn unsupported(color: ColorType, format: SampleFormat) -> Error {
    let name = match color {
        ColorType::Gray(_) | ColorType::Multiband { .. } => "grey",
        ColorType::RGB(_) | ColorType::RGBA(_) => "RGB",
        ColorType::CMYK(_) | ColorType::CMYKA(_) => "CMYK",
        ColorType::Palette(_) => "palette",
        ColorType::YCbCr(_) => "YCbCr",
        ColorType::Lab(_) => "Lab",
        _ => "colour",
    };
    let kind = match format {
        SampleFormat::Int => "signed ",
        SampleFormat::IEEEFP => "floating-point ",
        _ => "",
    };
    let bits = color.bit_depth();
    bad(format!(
        "its {bits}-bit {kind}{name} samples aren't supported"
    ))
}

/// Samples of `bits` bits each, packed from the high bit, every row starting on a new byte.
fn unpack(data: &[u8], bits: u8, w: u32, h: u32) -> Option<Vec<u8>> {
    let (bits, w) = (usize::from(bits), w as usize);
    let row = (w * bits).div_ceil(8);
    let mask = ((1u16 << bits) - 1) as u8;
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
