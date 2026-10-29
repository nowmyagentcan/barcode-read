//! PDF pages rendered to grey with hayro, one page in memory at a time.

use std::panic::{AssertUnwindSafe, catch_unwind};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Page as PdfPage;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
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
/// rendering the next. An encrypted file opens with `options.password`.
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
    for number in select(options.pages.as_ref(), all.len(), warnings)? {
        let page = &all[number - 1];
        let Some(dpi) = fit(options.dpi, page.render_dimensions()) else {
            warnings.push(Warning::page_failed(number, "it is too large to render"));
            continue;
        };
        if dpi < options.dpi {
            warnings.push(Warning::dpi_reduced(number, options.dpi, dpi));
        }
        match catch_unwind(AssertUnwindSafe(|| grey(page, &cache, dpi))) {
            Ok(grey) => each(Page {
                dpi: Some(dpi),
                ..Page::new(number, grey)
            }),
            Err(panic) => warnings.push(Warning::page_failed(number, panic_message(&*panic))),
        }
    }
    Ok(())
}

/// The highest whole DPI up to `dpi` at which a page of `w` x `h` points stays within the page
/// limits, sized as hayro sizes its pixmap.
fn fit(dpi: u32, (w, h): (f32, f32)) -> Option<u32> {
    (1..=dpi).rev().find(|&d| {
        let (pw, ph) = pixels(d, w, h);
        pw * ph <= MAX_PIXELS && pw.max(ph) <= MAX_SIDE
    })
}

fn pixels(dpi: u32, w: f32, h: f32) -> (u64, u64) {
    let scale = dpi as f32 / 72.0;
    ((w * scale) as u64, (h * scale) as u64)
}

/// Renders on white and converts to grey; the colour pixmap is freed on return.
fn grey<'a>(page: &'a PdfPage<'a>, cache: &RenderCache<'a>, dpi: u32) -> GrayImage {
    let scale = dpi as f32 / 72.0;
    let settings = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
    };
    let interpreter = InterpreterSettings::default();
    let pixmap = render(
        page,
        cache,
        &interpreter,
        &RenderSettings::default(),
        &settings,
    );
    let (w, h) = (pixmap.width().into(), pixmap.height().into());
    let rgba = ImageBuffer::<Rgba<u8>, _>::from_raw(w, h, pixmap.data_as_u8_slice());
    luma(&rgba.expect("four bytes per pixel"))
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
        let (w, h) = pixels(160, a0.0, a0.1);
        assert!(
            w * h <= MAX_PIXELS
                && pixels(161, a0.0, a0.1).0 * pixels(161, a0.0, a0.1).1 > MAX_PIXELS
        );
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
}
