//! Error and warning codes with the exact messages and hints that people and agents see.

use std::fmt::Display;

const USE_SUPPORTED: &str = "Use PNG, JPEG, GIF, BMP, WebP, TIFF, PNM or PDF.";

/// A file, usage or runtime error: a stable `code`, a sentence for people and an optional fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub hint: Option<&'static str>,
}

/// Input types that are recognised but not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    Heic,
    Avif,
    JpegXl,
    Jpeg2000,
    Svg,
    Other,
}

impl Error {
    fn new(code: &'static str, message: impl Into<String>, hint: Option<&'static str>) -> Self {
        Self {
            code,
            message: message.into(),
            hint,
        }
    }

    /// The process exit code this error forces: 2 for usage, 3 for runtime and internal, else 1.
    pub fn exit_code(&self) -> u8 {
        match self.code {
            "no_input" | "unknown_format" | "bad_pages" | "bad_dpi" => 2,
            "internal" | "port_in_use" | "io" => 3,
            _ => 1,
        }
    }

    pub fn not_found(path: &str) -> Self {
        Self::new(
            "not_found",
            format!("File not found: {path}"),
            Some("Check the path."),
        )
    }

    pub fn wildcard(path: &str) -> Self {
        Self::new(
            "wildcard",
            format!("No file named '{path}'."),
            Some(
                "Windows shells don't expand * or ?; pass the folder instead, e.g. barcode-read photos",
            ),
        )
    }

    pub fn unreadable(path: &str, cause: impl Display) -> Self {
        Self::new("unreadable", format!("Can't read {path}: {cause}."), None)
    }

    pub fn empty_file() -> Self {
        Self::new("empty_file", "The file is empty.", None)
    }

    pub fn too_large() -> Self {
        Self::new(
            "too_large",
            "The file is larger than 1 GiB.",
            Some("Split the PDF or shrink the image first."),
        )
    }

    pub fn unsupported_format(kind: Unsupported) -> Self {
        let (message, hint) = match kind {
            Unsupported::Heic => (
                "HEIC/HEIF photos aren't supported yet.",
                "Export it as JPEG first (iPhone: Settings > Camera > Formats > Most Compatible).",
            ),
            Unsupported::Avif => ("AVIF images aren't supported yet.", USE_SUPPORTED),
            Unsupported::JpegXl => ("JPEG XL images aren't supported yet.", USE_SUPPORTED),
            Unsupported::Jpeg2000 => ("JPEG 2000 images aren't supported yet.", USE_SUPPORTED),
            Unsupported::Svg => ("SVG files aren't supported.", "Render it to PNG first."),
            Unsupported::Other => ("This isn't a supported image or PDF.", USE_SUPPORTED),
        };
        Self::new("unsupported_format", message, Some(hint))
    }

    pub fn image_too_large(width: u64, height: u64) -> Self {
        Self::new(
            "image_too_large",
            format!(
                "The image is {width}x{height} pixels; the limit is 120 megapixels and 65,535 pixels per side."
            ),
            Some("Crop or resize it first."),
        )
    }

    /// `label` names the type as people know it ("PNG", "WebP"); `cause` is the decoder's message.
    pub fn corrupt_image(label: &str, cause: impl Display) -> Self {
        Self::new(
            "corrupt_image",
            format!("Can't decode this {label} file: {}.", sentence(cause)),
            Some("The file may be damaged or cut short."),
        )
    }

    pub fn corrupt_pdf(cause: impl Display) -> Self {
        Self::new(
            "corrupt_pdf",
            format!("Can't open this PDF: {}.", sentence(cause)),
            Some("The file may be damaged."),
        )
    }

    pub fn pdf_password() -> Self {
        Self::new(
            "pdf_password",
            "This PDF needs a password.",
            Some("Pass it with --password."),
        )
    }

    pub fn pdf_wrong_password() -> Self {
        Self::new("pdf_wrong_password", "The PDF password is wrong.", None)
    }

    pub fn pdf_encryption() -> Self {
        Self::new(
            "pdf_encryption",
            "This PDF uses an encryption method that isn't supported.",
            None,
        )
    }

    pub fn no_pages(list: &str, pages: usize) -> Self {
        Self::new(
            "no_pages",
            format!("--pages {list} selects none of this file's {pages} pages."),
            None,
        )
    }

    pub fn internal(cause: impl Display) -> Self {
        Self::new(
            "internal",
            format!(
                "Something went wrong while reading this file: {}.",
                sentence(cause)
            ),
            Some("Please report it, with the file if you can share it."),
        )
    }

    pub fn no_input() -> Self {
        Self::new(
            "no_input",
            "No file given.",
            Some("Try: barcode-read photo.jpg"),
        )
    }

    pub fn unknown_format(name: &str) -> Self {
        Self::new(
            "unknown_format",
            format!("Unknown barcode format '{name}'."),
            Some("Use names like qrcode, datamatrix, ean13, code128, all-linear or all-matrix."),
        )
    }

    pub fn bad_pages(list: &str) -> Self {
        Self::new(
            "bad_pages",
            format!("Can't read the page list '{list}'."),
            Some("Use page numbers and ranges like 1-3,7,10-"),
        )
    }

    pub fn bad_dpi() -> Self {
        Self::new("bad_dpi", "--dpi must be between 72 and 1200.", None)
    }

    pub fn port_in_use(port: u16) -> Self {
        Self::new(
            "port_in_use",
            format!("Can't start the page: port {port} is in use."),
            Some("Leave out --port to pick a free one."),
        )
    }

    pub fn io(cause: impl Display) -> Self {
        Self::new(
            "io",
            format!("Can't write output: {}.", sentence(cause)),
            None,
        )
    }
}

/// A note about a file that was still read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

impl Warning {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// `kind` is the file's JSON type name: PDFs and TIFFs get their own advice.
    pub fn none_found(kind: &str) -> Self {
        Self::new(
            "none_found",
            match kind {
                "pdf" => "No barcode found. For small codes try --dpi 600.",
                "tiff" => "No barcode found.",
                _ => "No barcode found. Try a sharper, closer photo with the whole code in view.",
            },
        )
    }

    pub fn dpi_reduced(page: usize, dpi: u32, used: u32) -> Self {
        Self::new(
            "dpi_reduced",
            format!("Page {page} is too large for {dpi} dpi, so it was read at {used} dpi."),
        )
    }

    pub fn pages_missing(pages: usize, list: &str) -> Self {
        Self::new(
            "pages_missing",
            format!("This file has {pages} pages; page {list} doesn't exist."),
        )
    }

    pub fn page_failed(page: usize, reason: impl Display) -> Self {
        Self::new(
            "page_failed",
            format!("Page {page} couldn't be read: {}.", sentence(reason)),
        )
    }

    pub fn first_frame_only() -> Self {
        Self::new(
            "first_frame_only",
            "Only the first frame of this animation was read.",
        )
    }
}

/// The message a panic was raised with.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    match (
        payload.downcast_ref::<&str>(),
        payload.downcast_ref::<String>(),
    ) {
        (Some(s), _) => s.to_string(),
        (_, Some(s)) => s.clone(),
        _ => "a panic without a message".into(),
    }
}

/// A cause as the middle of a sentence: no trailing full stop, so messages never end in "..".
fn sentence(cause: impl Display) -> String {
    cause
        .to_string()
        .trim_end()
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn causes_never_double_the_full_stop() {
        let e = Error::corrupt_image("PNG", "unexpected end of file.");
        assert_eq!(
            e.message,
            "Can't decode this PNG file: unexpected end of file."
        );
        assert_eq!(e.hint, Some("The file may be damaged or cut short."));
        assert_eq!(
            Warning::page_failed(2, "Group 3 fax compression isn't supported").message,
            "Page 2 couldn't be read: Group 3 fax compression isn't supported."
        );
    }

    #[test]
    fn usage_and_runtime_errors_have_their_own_exit_codes() {
        assert_eq!(Error::unknown_format("qr").exit_code(), 2);
        assert_eq!(Error::bad_dpi().exit_code(), 2);
        assert_eq!(Error::internal("boom").exit_code(), 3);
        assert_eq!(Error::port_in_use(8080).exit_code(), 3);
        assert_eq!(Error::not_found("a.png").exit_code(), 1);
        assert_eq!(Error::image_too_large(70000, 1).exit_code(), 1);
    }

    #[test]
    fn each_unsupported_type_has_its_own_message_and_hint() {
        let other = Some("Use PNG, JPEG, GIF, BMP, WebP, TIFF, PNM or PDF.");
        let heic =
            "Export it as JPEG first (iPhone: Settings > Camera > Formats > Most Compatible).";
        let cases = [
            (
                Unsupported::Heic,
                "HEIC/HEIF photos aren't supported yet.",
                Some(heic),
            ),
            (
                Unsupported::Avif,
                "AVIF images aren't supported yet.",
                other,
            ),
            (
                Unsupported::JpegXl,
                "JPEG XL images aren't supported yet.",
                other,
            ),
            (
                Unsupported::Jpeg2000,
                "JPEG 2000 images aren't supported yet.",
                other,
            ),
            (
                Unsupported::Svg,
                "SVG files aren't supported.",
                Some("Render it to PNG first."),
            ),
            (
                Unsupported::Other,
                "This isn't a supported image or PDF.",
                other,
            ),
        ];
        for (kind, message, hint) in cases {
            let e = Error::unsupported_format(kind);
            assert_eq!(
                (e.code, e.message.as_str(), e.hint),
                ("unsupported_format", message, hint)
            );
        }
    }

    #[test]
    fn none_found_advice_depends_on_the_type() {
        assert!(Warning::none_found("pdf").message.ends_with("--dpi 600."));
        assert_eq!(Warning::none_found("tiff").message, "No barcode found.");
        assert!(Warning::none_found("jpeg").message.contains("sharper"));
    }
}
