//! What kind of file an input is, from its first bytes (never from its name).

use crate::error::{Error, Unsupported};

/// How many leading bytes `sniff` looks at.
pub const HEAD: usize = 1024;

/// A readable input type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Jpeg,
    Png,
    Gif,
    Bmp,
    Webp,
    Tiff,
    Pnm,
}

impl Kind {
    /// The `type` value in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pdf => "pdf",
            Kind::Jpeg => "jpeg",
            Kind::Png => "png",
            Kind::Gif => "gif",
            Kind::Bmp => "bmp",
            Kind::Webp => "webp",
            Kind::Tiff => "tiff",
            Kind::Pnm => "pnm",
        }
    }

    /// The name people know the type by, for messages.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Pdf => "PDF",
            Kind::Jpeg => "JPEG",
            Kind::Png => "PNG",
            Kind::Gif => "GIF",
            Kind::Bmp => "BMP",
            Kind::Webp => "WebP",
            Kind::Tiff => "TIFF",
            Kind::Pnm => "PNM",
        }
    }
}

/// Identifies a file from its first bytes; the first matching signature wins.
pub fn sniff(bytes: &[u8]) -> Result<Kind, Error> {
    let head = &bytes[..bytes.len().min(HEAD)];
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    let at = |range: std::ops::Range<usize>| head.get(range).unwrap_or_default();
    let kind = match head {
        [] => return Err(Error::empty_file()),
        _ if has(b"%PDF-") => Kind::Pdf,
        [0xFF, 0xD8, 0xFF, ..] => Kind::Jpeg,
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Kind::Png,
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Kind::Gif,
        [b'B', b'M', ..] => Kind::Bmp,
        _ if at(0..4) == b"RIFF" && at(8..12) == b"WEBP" => Kind::Webp,
        [b'I', b'I', b'*', 0, ..] | [b'M', b'M', 0, b'*', ..] => Kind::Tiff,
        [b'P', b'1'..=b'6', c, ..] if c.is_ascii_whitespace() => Kind::Pnm,
        _ => {
            let brand = if at(4..8) == b"ftyp" { at(8..12) } else { b"" };
            let unsupported = match brand {
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                    Unsupported::Heic
                }
                b"avif" | b"avis" => Unsupported::Avif,
                _ if head.starts_with(b"\xFF\x0A") || head.starts_with(b"\0\0\0\x0CJXL ") => {
                    Unsupported::JpegXl
                }
                _ if head.starts_with(b"\0\0\0\x0CjP  ")
                    || head.starts_with(b"\xFF\x4F\xFF\x51") =>
                {
                    Unsupported::Jpeg2000
                }
                _ if has(b"<svg") => Unsupported::Svg,
                _ => Unsupported::Other,
            };
            return Err(Error::unsupported_format(unsupported));
        }
    };
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_supported_signature() {
        let cases: [(&[u8], Kind); 11] = [
            (b"%PDF-1.7\n", Kind::Pdf),
            (b"\xFF\xD8\xFF\xE0\0\x10JFIF", Kind::Jpeg),
            (b"\x89PNG\r\n\x1A\n\0\0\0\x0DIHDR", Kind::Png),
            (b"GIF87a\x01\0", Kind::Gif),
            (b"GIF89a\x01\0", Kind::Gif),
            (b"BM\x3A\0\0\0", Kind::Bmp),
            (b"RIFF\x24\0\0\0WEBPVP8 ", Kind::Webp),
            (b"II*\0\x08\0\0\0", Kind::Tiff),
            (b"MM\0*\0\0\0\x08", Kind::Tiff),
            (b"P5\n1 1\n255\n\0", Kind::Pnm),
            (b"P1 1 1 0", Kind::Pnm),
        ];
        for (bytes, kind) in cases {
            assert_eq!(sniff(bytes), Ok(kind), "{bytes:?}");
        }
    }

    #[test]
    fn finds_a_pdf_header_after_leading_junk_within_1024_bytes() {
        let mut bytes = vec![b' '; 1000];
        bytes.extend_from_slice(b"%PDF-1.4");
        assert_eq!(sniff(&bytes), Ok(Kind::Pdf));
        let mut late = vec![b' '; HEAD];
        late.extend_from_slice(b"%PDF-1.4");
        assert_eq!(
            sniff(&late),
            Err(Error::unsupported_format(Unsupported::Other))
        );
    }

    #[test]
    fn names_each_unsupported_type() {
        use Unsupported::*;
        let heic = [
            "heic", "heix", "hevc", "hevx", "heim", "heis", "mif1", "msf1",
        ]
        .map(|b| {
            (
                [b"\0\0\0\x18ftyp", b.as_bytes(), b"\0\0\0\0"].concat(),
                Heic,
            )
        });
        let cases: [(&[u8], Unsupported); 10] = [
            (b"\0\0\0\x1Cftypavif\0\0\0\0", Avif),
            (b"\0\0\0\x1Cftypavis\0\0\0\0", Avif),
            (b"\xFF\x0A\xFA\x7F", JpegXl),
            (b"\0\0\0\x0CJXL \r\n\x87\n", JpegXl),
            (b"\0\0\0\x0CjP  \r\n\x87\n", Jpeg2000),
            (b"\xFF\x4F\xFF\x51\0\x2F", Jpeg2000),
            (
                b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                Svg,
            ),
            (b"\0\0\0\x18ftypmp42", Other),
            (b"P7\nWIDTH 1\n", Other),
            (b"hello, world", Other),
        ];
        let cases = heic.iter().map(|(b, k)| (b.as_slice(), *k)).chain(cases);
        for (bytes, kind) in cases {
            assert_eq!(
                sniff(bytes),
                Err(Error::unsupported_format(kind)),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn an_empty_file_is_its_own_error() {
        assert_eq!(sniff(b"").unwrap_err().code, "empty_file");
    }
}
