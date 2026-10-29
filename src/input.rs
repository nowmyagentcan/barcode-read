//! The files to read: paths and folders expanded in order, their bytes loaded within the size
//! limit, and what kind of file each is, from its first bytes (never from its name).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::error::{Error, Unsupported, Warning};

/// How many leading bytes `sniff` looks at.
pub const HEAD: usize = 1024;

/// The largest file read, stdin included.
const MAX_FILE: u64 = 1 << 30;

/// The extensions a folder is read for; files named explicitly are read whatever their name.
const EXTENSIONS: [&str; 15] = [
    "png", "jpg", "jpeg", "jpe", "jfif", "gif", "bmp", "webp", "tif", "tiff", "pbm", "pgm", "ppm",
    "pnm", "pdf",
];

/// One file to read: its path as given (folder entries joined to the folder, stdin as `-`) and
/// where its bytes come from.
#[derive(Debug)]
pub struct Input {
    pub name: String,
    source: Source,
}

#[derive(Debug)]
enum Source {
    File(PathBuf),
    Stdin,
    /// A folder that couldn't be listed.
    Failed(Error),
}

/// The files `paths` name, in order: `-` is stdin, a folder gives its files with a readable
/// extension sorted by name, skipping hidden ones, and with `recursive` its subfolders in
/// place (depth-first; symlinked folders are not followed). A path given twice is read twice.
pub fn expand(paths: &[PathBuf], recursive: bool) -> Vec<Input> {
    let mut inputs = vec![];
    for path in paths {
        if path.as_os_str() == "-" {
            inputs.push(Input::new(path, Source::Stdin));
        } else if path.is_dir() {
            folder(path, recursive, &mut inputs);
        } else {
            inputs.push(Input::new(path, Source::File(path.clone())));
        }
    }
    inputs
}

fn folder(dir: &Path, recursive: bool, inputs: &mut Vec<Input>) {
    let entries = fs::read_dir(dir).and_then(|list| list.collect::<io::Result<Vec<_>>>());
    let mut entries = match entries {
        Ok(entries) => entries,
        Err(e) => {
            let name = dir.to_string_lossy();
            let error = Error::unreadable(&name, e);
            return inputs.push(Input::new(dir, Source::Failed(error)));
        }
    };
    entries.sort_by(|a, b| {
        let name = |e: &fs::DirEntry| e.file_name().into_encoded_bytes();
        name(a).cmp(&name(b))
    });
    for entry in entries {
        let path = entry.path();
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        // A symlink is followed to a file, never to a folder.
        let is_link = entry.file_type().is_ok_and(|t| t.is_symlink());
        if path.is_dir() {
            if recursive && !is_link {
                folder(&path, recursive, inputs);
            }
        } else if path
            .extension()
            .is_some_and(|ext| EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
        {
            inputs.push(Input::new(&path, Source::File(path.clone())));
        }
    }
}

impl Input {
    fn new(path: &Path, source: Source) -> Self {
        Self {
            name: path.to_string_lossy().into_owned(),
            source,
        }
    }

    /// The whole file, refused before reading when it is over 1 GiB.
    pub fn load(&self) -> Result<Vec<u8>, Error> {
        let name = self.name.as_str();
        let unreadable = |e| Error::unreadable(name, e);
        let (reader, size): (Box<dyn Read>, u64) = match &self.source {
            Source::Failed(e) => return Err(e.clone()),
            Source::Stdin => (Box::new(io::stdin().lock()), 0),
            Source::File(path) => {
                let file = File::open(path).map_err(|e| match e.kind() {
                    _ if fs::metadata(path).is_ok() => unreadable(e),
                    _ if name.contains(['*', '?']) => Error::wildcard(name),
                    io::ErrorKind::NotFound => Error::not_found(name),
                    _ => unreadable(e),
                })?;
                let size = file.metadata().map_err(unreadable)?.len();
                if size > MAX_FILE {
                    return Err(Error::too_large());
                }
                (Box::new(file), size)
            }
        };
        let mut bytes = Vec::with_capacity(size as usize);
        reader
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(Error::too_large());
        }
        Ok(bytes)
    }
}

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

/// A `--pages` list such as `1-3,7,10-`: 1-based pages and ranges, where an open end means the
/// last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageList {
    /// As given, for messages.
    text: String,
    ranges: Vec<(usize, Option<usize>)>,
}

impl PageList {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let bad = || Error::bad_pages(text);
        let number = |s: &str| {
            let s = s.trim();
            let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            s.parse().ok().filter(|&n| digits && n > 0).ok_or_else(bad)
        };
        let range = |item: &str| match item.split_once('-') {
            None => number(item).map(|n| (n, Some(n))),
            Some((from, to)) if to.trim().is_empty() => Ok((number(from)?, None)),
            Some((from, to)) => {
                let (from, to) = (number(from)?, number(to)?);
                (from <= to).then_some((from, Some(to))).ok_or_else(bad)
            }
        };
        let ranges = text.split(',').map(range).collect::<Result<_, _>>()?;
        Ok(Self {
            text: text.into(),
            ranges,
        })
    }
}

/// The pages to read, in order and once each, of a file with `count` pages: all of them, or
/// those `list` selects. Listed pages past the end add a `pages_missing` warning; a list that
/// selects none of the pages is the error `no_pages`.
pub fn select(
    list: Option<&PageList>,
    count: usize,
    warnings: &mut Vec<Warning>,
) -> Result<Vec<usize>, Error> {
    let Some(list) = list else {
        return Ok((1..=count).collect());
    };
    let selected = |p: usize| {
        list.ranges
            .iter()
            .any(|&(a, b)| a <= p && b.is_none_or(|b| p <= b))
    };
    let pages: Vec<_> = (1..=count).filter(|&p| selected(p)).collect();
    if pages.is_empty() {
        return Err(Error::no_pages(&list.text, count));
    }
    let past = count + 1;
    let missing: Vec<_> = list
        .ranges
        .iter()
        .filter_map(|&(a, b)| match b {
            None => (a > count).then(|| format!("{a}-")),
            Some(b) if b < past => None,
            Some(b) if a.max(past) == b => Some(b.to_string()),
            Some(b) => Some(format!("{}-{b}", a.max(past))),
        })
        .collect();
    if !missing.is_empty() {
        warnings.push(Warning::pages_missing(count, &missing.join(",")));
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder holding empty files at `files` (subfolders made as needed).
    fn tree(test: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("barcode-read-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            File::create(path).unwrap();
        }
        root
    }

    fn names(inputs: &[Input], root: &Path) -> Vec<String> {
        let root = root.to_string_lossy();
        let rel = |i: &Input| i.name[root.len() + 1..].replace('\\', "/");
        inputs.iter().map(rel).collect()
    }

    #[test]
    fn a_folder_gives_its_images_in_name_order() {
        let files = [
            "b.PNG",
            "a.pdf",
            "B.jpg",
            "notes.txt",
            ".hidden.png",
            "sub/c.png",
            "sub/.d/e.png",
            "x.tiff",
        ];
        let root = tree("folder", &files);
        let dir = std::slice::from_ref(&root);
        let flat = expand(dir, false);
        assert_eq!(names(&flat, &root), ["B.jpg", "a.pdf", "b.PNG", "x.tiff"]);
        let deep = expand(dir, true);
        assert_eq!(
            names(&deep, &root),
            ["B.jpg", "a.pdf", "b.PNG", "sub/c.png", "x.tiff"]
        );
        let twice = expand(&[root.join("a.pdf"), "-".into(), root.join("a.pdf")], false);
        assert_eq!(names(&twice[..1], &root), ["a.pdf"]);
        assert_eq!(twice[1].name, "-");
        assert_eq!(twice[2].name, twice[0].name);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folders_are_not_followed() {
        let root = tree("links", &["real/a.png", "dir/b.png"]);
        std::os::unix::fs::symlink(root.join("real"), root.join("dir/link")).unwrap();
        std::os::unix::fs::symlink(root.join("real/a.png"), root.join("dir/c.png")).unwrap();
        let read = expand(&[root.join("dir")], true);
        assert_eq!(names(&read, &root), ["dir/b.png", "dir/c.png"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_files_are_not_found_or_a_wildcard() {
        let load = |name: &str| expand(&[name.into()], false)[0].load().unwrap_err();
        assert_eq!(load("no-such.png"), Error::not_found("no-such.png"));
        assert_eq!(load("photos/*.png"), Error::wildcard("photos/*.png"));
        let root = tree("load", &["a.png"]);
        let a = root.join("a.png");
        assert_eq!(expand(&[a], false)[0].load(), Ok(vec![]));
        fs::remove_dir_all(root).unwrap();
    }

    fn pages(list: &str, count: usize) -> (Result<Vec<usize>, Error>, Vec<Warning>) {
        let mut warnings = vec![];
        let list = PageList::parse(list).unwrap();
        (select(Some(&list), count, &mut warnings), warnings)
    }

    #[test]
    fn page_lists_take_numbers_ranges_and_open_ends() {
        assert_eq!(pages("1-3,7,10-", 12).0, Ok(vec![1, 2, 3, 7, 10, 11, 12]));
        assert_eq!(pages(" 3 , 1-2,2 ", 5).0, Ok(vec![1, 2, 3]));
        assert_eq!(select(None, 3, &mut vec![]), Ok(vec![1, 2, 3]));
        for bad in [
            "", "0", "1,,2", "3-1", "-3", "1-2-3", "a", "+1", "1.5", "1 2",
        ] {
            assert_eq!(PageList::parse(bad), Err(Error::bad_pages(bad)), "{bad:?}");
        }
        assert_eq!(
            Error::bad_pages("x").hint,
            Some("Use page numbers and ranges like 1-3,7,10-")
        );
    }

    #[test]
    fn pages_past_the_end_warn_and_selecting_none_fails() {
        let (read, warnings) = pages("2-9", 3);
        assert_eq!(read, Ok(vec![2, 3]));
        assert_eq!(
            warnings[0].message,
            "This file has 3 pages; page 4-9 doesn't exist."
        );
        assert_eq!(
            pages("1,4,6-,2-4", 3).1,
            [Warning::pages_missing(3, "4,6-,4")]
        );
        assert!(pages("1-3,3-", 3).1.is_empty());
        let (read, _) = pages("9", 3);
        let e = read.unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            ("no_pages", "--pages 9 selects none of this file's 3 pages.")
        );
    }

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
