//! PDF pages rendered to grey with hayro, one page in memory at a time.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flate2::{Decompress, FlushDecompress, Status};
use hayro::hayro_interpret::{InterpreterSettings, InterpreterWarning};
use hayro::hayro_syntax::content::TypedIter;
use hayro::hayro_syntax::content::ops::TypedInstruction;
use hayro::hayro_syntax::object::stream::ImageDecodeParams;
use hayro::hayro_syntax::object::{Array, Dict, Name, Object, ObjectIdentifier, Stream};
use hayro::hayro_syntax::page::{Page as PdfPage, Resources};
use hayro::hayro_syntax::{DecryptionError, Filter, LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use image::{GrayImage, ImageBuffer, Rgba};

use crate::Options;
use crate::decode::Page;
use crate::error::{Error, Warning, panic_message};
use crate::input::select;
use crate::raster::luma;

/// A rendered page stays within 40 megapixels and 65,535 pixels per side.
const MAX_PIXELS: u64 = 40_000_000;
const MAX_SIDE: u64 = 65_535;

/// Renders the selected pages (`options.pages`) in order at `options.dpi`, or at the highest
/// whole DPI that fits the page limits (with a warning), and hands each to `each` before
/// rendering the next. An encrypted file opens with `options.password`. A page whose content
/// can't be decoded or found fails; when no selected page renders, the file fails.
pub fn pages(
    bytes: Vec<u8>,
    options: &Options,
    warnings: &mut Vec<Warning>,
    mut each: impl FnMut(Page),
) -> Result<(), Error> {
    let password = options.password.as_deref();
    let pdf = Pdf::new_with_password(bytes, password.unwrap_or("")).map_err(|e| match e {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_some() => {
            Error::pdf_wrong_password()
        }
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => Error::pdf_password(),
        LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => Error::pdf_encryption(),
        LoadPdfError::Decryption(_) => Error::corrupt_pdf("its encryption settings are damaged"),
        LoadPdfError::Invalid => Error::corrupt_pdf("its structure can't be read"),
    })?;
    let cache = RenderCache::new();
    let all = pdf.pages();
    let (mut rendered, mut failed) = (false, false);
    for number in select(options.pages.as_ref(), all.len(), warnings)? {
        let page = &all[number - 1];
        let Some(dpi) = fit(options.dpi, page.render_dimensions()) else {
            warnings.push(Warning::page_failed(number, "it is too large to render"));
            failed = true;
            continue;
        };
        if dpi < options.dpi {
            warnings.push(Warning::dpi_reduced(number, options.dpi, dpi));
        }
        match catch_unwind(AssertUnwindSafe(|| grey(page, &cache, dpi))) {
            Ok(None) => {
                let reason = "part of its content is damaged or missing";
                warnings.push(Warning::page_failed(number, reason));
                failed = true;
            }
            Ok(Some(grey)) => {
                rendered = true;
                each(Page {
                    dpi: Some(dpi),
                    ..Page::new(number, grey)
                });
            }
            Err(panic) => {
                warnings.push(Warning::page_failed(number, panic_message(&*panic)));
                failed = true;
            }
        }
    }
    if !rendered {
        // Every selected page fails or renders, so none failing means none was selected.
        let cause = if failed {
            "no page could be rendered"
        } else {
            "it has no pages"
        };
        return Err(Error::corrupt_pdf(cause));
    }
    Ok(())
}

/// The highest whole DPI up to `dpi` at which a page of `w` x `h` points stays within the page
/// limits, sized as hayro sizes its pixmap.
fn fit(dpi: u32, (w, h): (f32, f32)) -> Option<u32> {
    (1..=dpi).rev().find(|&d| {
        let (pw, ph) = pixels(d, w, h);
        pw.saturating_mul(ph) <= MAX_PIXELS && pw.max(ph) <= MAX_SIDE
    })
}

fn pixels(dpi: u32, w: f32, h: f32) -> (u64, u64) {
    let scale = dpi as f32 / 72.0;
    ((w * scale) as u64, (h * scale) as u64)
}

/// Renders on white and converts to grey, or `None` when content is lost (`intact`) or an
/// image fails to decode. The colour pixmap is freed on return.
fn grey<'a>(page: &'a PdfPage<'a>, cache: &RenderCache<'a>, dpi: u32) -> Option<GrayImage> {
    if !intact(page) {
        return None;
    }
    let scale = dpi as f32 / 72.0;
    let settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
    };
    let failed = Arc::new(AtomicBool::new(false));
    let sink = failed.clone();
    let interpreter = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ImageDecodeFailure) {
                sink.store(true, Ordering::Relaxed);
            }
        }),
        ..InterpreterSettings::default()
    };
    let pixmap = render(
        page,
        cache,
        &interpreter,
        &RenderSettings::default(),
        &settings,
    );
    if failed.load(Ordering::Relaxed) {
        return None;
    }
    let (w, h) = (pixmap.width().into(), pixmap.height().into());
    let rgba = ImageBuffer::<Rgba<u8>, _>::from_raw(w, h, pixmap.data_as_u8_slice());
    Some(luma(&rgba.expect("four bytes per pixel")))
}

/// Whether everything the page draws is there and whole. hayro leaves out what it can't find
/// or decode, and decodes damaged or cut-short data as far as it goes, without an error; this
/// checks each /Contents stream, every XObject a `Do` names (forms followed down) and each
/// annotation's appearance. No /Contents, or null, is a blank page.
fn intact(page: &PdfPage) -> bool {
    let contents = match entry(page.raw(), b"Contents") {
        Ok(None | Some(Object::Null(_))) => vec![],
        Ok(Some(Object::Stream(s))) => vec![s],
        Ok(Some(Object::Array(a))) => {
            let all: Vec<_> = a.iter::<Stream>().collect();
            // The iterator stops at a member that is missing or isn't a stream.
            if all.len() < a.raw_iter().count() {
                return false;
            }
            all
        }
        _ => return false,
    };
    let mut seen = HashSet::new();
    let resources = page.resources();
    contents.iter().all(|s| whole(s) && s.decoded().is_ok())
        && draws(page.typed_operations(), resources, &mut seen, 0)
        && appearances(page.raw()).all(|ap| match ap {
            Ok(Some(Object::Stream(s))) => form(&s, resources, &mut seen, 0),
            Ok(_) => true,
            Err(()) => false,
        })
}

/// `dict[key]`, or `Err` when it is there but can't be read (a missing object, say).
fn entry<'a>(dict: &Dict<'a>, key: &[u8]) -> Result<Option<Object<'a>>, ()> {
    match dict.get::<Object>(key) {
        None if dict.contains_key(key) => Err(()),
        found => Ok(found),
    }
}

/// The normal appearance hayro draws for each shown annotation: /AP /N, or its /AS or /Off state.
fn appearances<'a>(page: &Dict<'a>) -> impl Iterator<Item = Result<Option<Object<'a>>, ()>> {
    let annots = page.get::<Array>("Annots").into_iter();
    annots.flat_map(|a| a.iter::<Dict>()).filter_map(|annot| {
        let shown = annot.get::<u32>("F").unwrap_or(0) & 2 == 0;
        let ap = annot.get::<Dict>("AP").filter(|_| shown)?;
        Some(match entry(&ap, b"N") {
            Ok(Some(Object::Dict(states))) => {
                let state = annot.get::<Name>("AS");
                entry(&states, state.as_deref().unwrap_or(b"Off"))
            }
            n => n,
        })
    })
}

/// Whether every XObject the content `ops` draws is there and whole.
fn draws<'a>(
    mut ops: TypedIter,
    resources: &Resources<'a>,
    seen: &mut HashSet<ObjectIdentifier>,
    depth: u32,
) -> bool {
    while let Some(op) = ops.next() {
        if let TypedInstruction::XObject(x) = op {
            let Some(s) = resources.get_x_object(x.0) else {
                return false;
            };
            // Each object is checked once, which also ends a form that draws itself.
            let ok = !seen.insert(s.obj_id())
                || match s.dict().get::<Name>("Subtype").as_deref() {
                    Some(b"Form") => form(&s, resources, seen, depth),
                    _ => whole(&s),
                };
            if !ok {
                return false;
            }
        }
    }
    true
}

/// Whether a form is whole and draws only what is there, up to hayro's nesting limit.
fn form<'a>(
    s: &Stream<'a>,
    parent: &Resources<'a>,
    seen: &mut HashSet<ObjectIdentifier>,
    depth: u32,
) -> bool {
    let Some(data) = s.decoded().ok().filter(|_| whole(s)) else {
        return false;
    };
    let own = s.dict().get::<Dict>("Resources").map(Resources::new);
    depth >= 50
        || draws(
            TypedIter::new(&data),
            own.as_ref().unwrap_or(parent),
            seen,
            depth + 1,
        )
}

/// Whether a stream's data is whole: each filter is one PDF defines, and Flate or RunLength
/// data reaches its end, a JPEG its end marker and a fax image all its rows. Empty data is
/// whole.
fn whole(s: &Stream) -> bool {
    let named = match s.dict().get::<Object>("Filter") {
        Some(Object::Array(a)) => a.raw_iter().count(),
        Some(_) => 1,
        None => 0,
    };
    let (filters, data) = (s.filters(), s.raw_data());
    filters.len() == named
        && (data.trim_ascii().is_empty()
            || match filters.as_slice() {
                [Filter::FlateDecode, ..] => inflates(&data),
                [Filter::RunLengthDecode, ..] => runs_to_end(&data),
                [Filter::DctDecode] => jpeg_ends(&data),
                [Filter::CcittFaxDecode] => all_rows(s),
                _ => true,
            })
}

/// Whether zlib or bare deflate data runs to its final block (readers ignore the checksum).
fn inflates(data: &[u8]) -> bool {
    let header = u16::from_be_bytes([data[0], *data.get(1).unwrap_or(&0)]);
    let zlib = header & 0x0F00 == 0x0800 && header.is_multiple_of(31);
    let data = data.get(if zlib { 2 } else { 0 }..).unwrap_or_default();
    let (mut inflate, mut out) = (Decompress::new(false), vec![0; 1 << 16]);
    loop {
        let (i, o) = (inflate.total_in(), inflate.total_out());
        match inflate.decompress(&data[i as usize..], &mut out, FlushDecompress::None) {
            Ok(Status::StreamEnd) => return true,
            Ok(_) if inflate.total_in() > i || inflate.total_out() > o => {}
            _ => return false,
        }
    }
}

/// Whether RunLength data reaches its end-of-data byte.
fn runs_to_end(data: &[u8]) -> bool {
    let mut at = 0;
    while let Some(&n) = data.get(at) {
        match n {
            128 => return true,
            0..128 => at += usize::from(n) + 2,
            _ => at += 2,
        }
    }
    false
}

/// Whether a fax image decodes to all the rows its dictionary gives.
fn all_rows(s: &Stream) -> bool {
    let size = |key| s.dict().get::<u32>(key).unwrap_or(0);
    let params = ImageDecodeParams {
        width: size("Width"),
        height: size("Height"),
        ..ImageDecodeParams::default()
    };
    s.decoded_image(&params).is_ok_and(|r| {
        let size = r
            .image_data
            .map(|i| u64::from(i.width) * u64::from(i.height));
        size.is_none_or(|size| r.data.len() as u64 >= size)
    })
}

/// Whether a JPEG reaches its end marker. Past the segments before the first scan (where an
/// EXIF thumbnail keeps its own end marker), FF D9 only ever ends the image.
fn jpeg_ends(data: &[u8]) -> bool {
    let mut at = 2;
    loop {
        match data.get(at..at + 4) {
            Some(&[0xFF, 0xFF, _, _]) => at += 1,
            Some(&[0xFF, 0xDA, _, _]) => return data[at..].windows(2).any(|w| w == [0xFF, 0xD9]),
            Some(&[0xFF, _, hi, lo]) => at += 2 + usize::from(u16::from_be_bytes([hi, lo])),
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_the_dpi_until_the_page_fits() {
        let a4 = (595.0, 842.0);
        assert_eq!(fit(300, a4), Some(300));
        assert_eq!(pixels(300, a4.0, a4.1), (2479, 3508));
        // A0 at 300 dpi would be 139 megapixels.
        let a0 = (2384.0, 3370.0);
        assert_eq!(fit(300, a0), Some(160));
        let ((w, h), (w2, h2)) = (pixels(160, a0.0, a0.1), pixels(161, a0.0, a0.1));
        assert!(w * h <= MAX_PIXELS && w2 * h2 > MAX_PIXELS);
        // A long strip is bound by its side instead.
        assert_eq!(fit(1200, (14_400.0, 10.0)), Some(327));
        assert_eq!(fit(300, (1e9, 1e9)), None);
    }

    #[test]
    fn a_damaged_file_is_corrupt_pdf() {
        let bytes = b"%PDF-1.7\nnot really".to_vec();
        let e = pages(bytes, &Options::default(), &mut vec![], |_| ()).unwrap_err();
        assert_eq!(e.code, "corrupt_pdf");
        assert_eq!(e.hint, Some("The file may be damaged."));
    }

    #[test]
    fn a_lone_zlib_header_byte_is_not_whole() {
        // 0xF8 padded with 0 passes the zlib header test, though no header byte follows.
        assert!(!inflates(&[0xF8]));
    }
}
