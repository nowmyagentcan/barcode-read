//! Files with pages: PDF pages rendered at a chosen resolution, and TIFF pages.

mod common;

use barcode_read::Options;
use barcode_read::error::Warning;
use common::{pdf_file, read_pages, vector_qr};

const A4: (f32, f32) = (595.0, 842.0);

/// Three A4 pages; page 2 has a QR whose top-left corner is 100 points in from the left and top.
fn three_pages() -> Vec<u8> {
    let blank = (A4.0, A4.1, String::new());
    let qr = (A4.0, A4.1, vector_qr("page two", 100.0, 100.0, 3.0, A4.1));
    pdf_file(&[blank.clone(), qr, blank])
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
    let bytes = pdf_file(&[(a0.0, a0.1, vector_qr("a0 poster", 200.0, 200.0, 6.0, a0.1))]);
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
