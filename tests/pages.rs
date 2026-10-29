//! Files with pages: PDF pages rendered at a chosen resolution, and TIFF pages.

mod common;

use barcode_read::Options;
use barcode_read::decode::Page;
use barcode_read::error::Warning;
use barcode_read::input::PageList;
use barcode_read::{engine, pdf, tiff};
use common::{pdf_file, read_pages, vector_qr};

const A4: (f32, f32) = (595.0, 842.0);

/// Three A4 pages; page 2 has a QR whose top-left corner is 100 points in from the left and top.
fn three_pages() -> Vec<u8> {
    let blank = (A4.0, A4.1, String::new());
    let qr = (A4.0, A4.1, vector_qr("page two", 100.0, 100.0, 3.0, A4.1));
    pdf_file(&[blank.clone(), qr, blank], "")
}

#[test]
fn reads_a_vector_qr_on_page_two_at_300_dpi() {
    let read = read_pages(&three_pages(), &Options::default());
    let size = (2479, 3508);
    assert_eq!(
        read.pages,
        [
            (1, size, Some(300)),
            (2, size, Some(300)),
            (3, size, Some(300))
        ]
    );
    assert_eq!(read.codes.len(), 1);
    let c = &read.codes[0];
    assert_eq!((c.page, c.text.as_str()), (2, "page two"));
    // The symbol starts after the 4-module quiet zone: 112 points, 467 pixels at 300 dpi.
    let [x, y] = c.position[0];
    assert!(
        (x - 467).abs() <= 1 && (y - 467).abs() <= 1,
        "{:?}",
        c.position
    );
}

#[test]
fn a_lower_dpi_halves_the_coordinates() {
    let options = Options {
        dpi: 150,
        ..Options::default()
    };
    let read = read_pages(&three_pages(), &options);
    assert_eq!(read.pages[1], (2, (1239, 1754), Some(150)));
    let [x, y] = read.codes[0].position[0];
    assert!(
        (x - 233).abs() <= 1 && (y - 233).abs() <= 1,
        "{:?}",
        read.codes[0].position
    );
}

#[test]
fn reads_an_a0_page_at_a_reduced_dpi() {
    let a0 = (2384.0, 3370.0);
    let bytes = pdf_file(
        &[(a0.0, a0.1, vector_qr("a0 poster", 200.0, 200.0, 6.0, a0.1))],
        "",
    );
    let read = read_pages(&bytes, &Options::default());
    assert_eq!(read.pages, [(1, (5297, 7488), Some(160))]);
    assert_eq!(read.codes[0].text, "a0 poster");
    assert_eq!(
        read.warnings,
        [Warning {
            code: "dpi_reduced",
            message: "Page 1 is too large for 300 dpi, so it was read at 160 dpi.".into()
        }]
    );
}

/// A page too large to render at any DPI: a billion points square.
const HUGE: (f32, f32) = (1e9, 1e9);

#[test]
fn a_page_too_large_to_render_leaves_the_others() {
    let huge = (HUGE.0, HUGE.1, String::new());
    let qr = (A4.0, A4.1, vector_qr("page two", 100.0, 100.0, 3.0, A4.1));
    let read = read_pages(&pdf_file(&[huge, qr], ""), &Options::default());
    assert_eq!(read.pages.iter().map(|p| p.0).collect::<Vec<_>>(), [2]);
    assert_eq!(read.codes[0].text, "page two");
    assert_eq!(
        read.warnings,
        [Warning {
            code: "page_failed",
            message: "Page 1 couldn't be read: it is too large to render.".into()
        }]
    );
}

#[test]
fn a_pdf_with_no_page_rendered_is_an_error() {
    let huge = (HUGE.0, HUGE.1, String::new());
    let pdf = pdf_file(&[huge.clone(), huge], "");
    let qr = pdf_file(
        &[(A4.0, A4.1, vector_qr("ok", 100.0, 100.0, 3.0, A4.1))],
        "",
    );
    let files = [
        engine::read("ok.pdf".into(), qr, &Options::default()),
        engine::read("huge.pdf".into(), pdf, &Options::default()),
    ];
    let report = engine::Report::new(&files);
    assert!(files[1].error.is_some(), "{:?}", files[1].warnings);
    assert_eq!((report.failed, report.exit_code()), (1, 1));
}

fn with_pages(list: &str) -> Options {
    Options {
        pages: Some(PageList::parse(list).unwrap()),
        ..Options::default()
    }
}

#[test]
fn reads_only_the_chosen_pages() {
    let read = read_pages(&three_pages(), &with_pages("2"));
    assert_eq!(read.pages.iter().map(|p| p.0).collect::<Vec<_>>(), [2]);
    assert_eq!(read.codes[0].page, 2);
    let read = read_pages(&three_pages(), &with_pages("2-9"));
    assert_eq!(read.pages.len(), 2);
    assert_eq!(read.warnings[0].code, "pages_missing");
}

#[test]
fn a_list_of_pages_past_the_end_is_an_error() {
    let mut warnings = vec![];
    let e = pdf::pages(three_pages(), &with_pages("9"), &mut warnings, |_| ()).unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        ("no_pages", "--pages 9 selects none of this file's 3 pages.")
    );
}

#[test]
fn an_unsupported_security_handler_is_pdf_encryption() {
    let page = (A4.0, A4.1, String::new());
    let encrypt = "/Encrypt << /Filter /Adobe.PubSec /V 4 /R 4 >> /ID [<00> <00>]";
    let e = pdf::pages(
        pdf_file(&[page], encrypt),
        &Options::default(),
        &mut vec![],
        |_| (),
    )
    .unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            "pdf_encryption",
            "This PDF uses an encryption method that isn't supported."
        )
    );
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn texts(read: &common::Read) -> Vec<(usize, &str)> {
    read.codes
        .iter()
        .map(|c| (c.page, c.text.as_str()))
        .collect()
}

#[test]
fn reads_a_group_4_fax_page_by_page() {
    let read = read_pages(&fixture("fax.tif"), &Options::default());
    let pages: Vec<_> = read.pages.iter().map(|p| (p.0, p.1)).collect();
    assert_eq!(pages, [(1, (240, 240)), (2, (240, 240)), (3, (240, 240))]);
    assert_eq!(texts(&read), [(2, "fax page 2")]);
    assert!(read.warnings.is_empty());
    // Min-is-white: blank paper reads as white, not inverted.
    let mut blank = vec![];
    let each = |p: Page| blank.push(p.grey.iter().all(|&v| v == 255));
    tiff::pages(&fixture("fax.tif"), &Options::default(), &mut vec![], each).unwrap();
    assert_eq!(blank, [true, false, true]);
    let read = read_pages(&fixture("fax.tif"), &with_pages("2-9"));
    assert_eq!(texts(&read), [(2, "fax page 2")]);
    assert_eq!(
        read.warnings[0].message,
        "This file has 3 pages; page 4-9 doesn't exist."
    );
}

#[test]
fn a_group_3_page_fails_and_the_next_still_reads() {
    let read = read_pages(&fixture("fax-g3.tif"), &Options::default());
    assert_eq!(texts(&read), [(2, "group 4")]);
    assert_eq!(
        read.warnings,
        [Warning {
            code: "page_failed",
            message: "Page 1 couldn't be read: Group 3 fax compression isn't supported.".into()
        }]
    );
}

#[test]
fn reads_scanned_pages_in_every_pdf_image_codec() {
    let bytes = fixture("scan.pdf");
    let codecs = [
        "DCTDecode",
        "FlateDecode",
        "CCITTFaxDecode",
        "JBIG2Decode",
        "JPXDecode",
    ];
    for codec in codecs {
        let name = format!("/{codec}");
        assert!(
            bytes.windows(name.len()).any(|w| w == name.as_bytes()),
            "{codec}"
        );
    }
    let read = read_pages(&bytes, &Options::default());
    let expected: Vec<_> = codecs
        .iter()
        .enumerate()
        .map(|(i, c)| (i + 1, *c))
        .collect();
    assert_eq!(texts(&read), expected);
    // Each page's paper is white: no codec's image comes out inverted.
    let mut corners = vec![];
    let each = |p: Page| corners.push(p.grey[(3, 3)].0[0]);
    pdf::pages(bytes, &Options::default(), &mut vec![], each).unwrap();
    assert!(corners.iter().all(|&v| v > 230), "{corners:?}");
}

#[test]
fn an_aes_pdf_opens_only_with_its_password() {
    let open = |password: Option<&str>| {
        let options = Options {
            password: password.map(String::from),
            ..Options::default()
        };
        pdf::pages(fixture("aes.pdf"), &options, &mut vec![], |_| ())
    };
    let e = open(None).unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str(), e.hint),
        (
            "pdf_password",
            "This PDF needs a password.",
            Some("Pass it with --password.")
        )
    );
    let e = open(Some("sesame")).unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        ("pdf_wrong_password", "The PDF password is wrong.")
    );
    let options = Options {
        password: Some("open-sesame".into()),
        ..Options::default()
    };
    assert_eq!(
        texts(&read_pages(&fixture("aes.pdf"), &options)),
        [(1, "secret page")]
    );
}
