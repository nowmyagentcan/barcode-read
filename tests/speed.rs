//! The speed targets, timed on a release build: `cargo test --release -- --ignored --nocapture
//! speed`. Each time is the median of 10 runs and includes starting the process, except the
//! page's request, which goes to a server already running.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{code, pdf_file, vector_qr};
use image::codecs::jpeg::JpegEncoder;
use image::{GrayImage, Luma, imageops};
use zxingcpp::BarcodeFormat;

const BIN: &str = env!("CARGO_BIN_EXE_barcode-read");

fn median(mut run: impl FnMut()) -> Duration {
    let mut times: Vec<_> = (0..10)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .collect();
    times.sort();
    times[5]
}

/// Runs the CLI and checks how many codes it found.
fn cli(args: &[&str], found: usize) {
    let out = Command::new(BIN).args(args).arg("--json").output().unwrap();
    let json = String::from_utf8_lossy(&out.stdout);
    assert!(
        json.ends_with(&format!(",\"found\":{found},\"failed\":0}}\n")),
        "{json}"
    );
}

/// A grey photo-like picture: soft shading with sensor noise of `noise` levels.
fn photo(width: u32, height: u32, noise: u32) -> GrayImage {
    let mut seed = 0x2545_f491_u32;
    GrayImage::from_fn(width, height, |x, y| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let shade = 90 + (x * 80 / width + y * 60 / height) as u8;
        Luma([shade + (seed % noise) as u8])
    })
}

/// A QR of `text` at `scale` times zxing-cpp's 4 pixels per module.
fn qr(text: &str, scale: u32) -> GrayImage {
    let img = code(BarcodeFormat::QRCode, text, "");
    let (w, h) = img.dimensions();
    imageops::resize(&img, w * scale, h * scale, imageops::FilterType::Nearest)
}

fn jpeg(img: &GrayImage) -> Vec<u8> {
    let mut bytes = vec![];
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, 90))
        .unwrap();
    bytes
}

/// An A4 PDF with one 300 dpi greyscale JPEG scan per page.
fn scanned_pdf(scans: &[Vec<u8>]) -> Vec<u8> {
    let kids: Vec<_> = (0..scans.len())
        .map(|i| format!("{} 0 R", 3 + 3 * i))
        .collect();
    let pages = format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        scans.len()
    );
    let mut objects: Vec<Vec<u8>> =
        vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), pages.into()];
    let draw = "q 595 0 0 842 0 0 cm /Im0 Do Q";
    for (i, scan) in scans.iter().enumerate() {
        let (contents, image) = (4 + 3 * i, 5 + 3 * i);
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents {contents} 0 R /Resources << /XObject << /Im0 {image} 0 R >> >> >>").into());
        objects.push(format!("<< /Length {} >>\nstream\n{draw}\nendstream", draw.len()).into());
        let mut img = format!("<< /Type /XObject /Subtype /Image /Width 2480 /Height 3508 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", scan.len()).into_bytes();
        img.extend(scan);
        img.extend(b"\nendstream");
        objects.push(img);
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for (i, object) in objects.iter().enumerate() {
        xref += &format!("{:010} 00000 n \n", out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend(object);
        out.extend(b"\nendobj\n");
    }
    let start = out.len();
    out.extend(xref.bytes());
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

#[test]
#[ignore = "times a release build: cargo test --release -- --ignored speed"]
fn meets_the_speed_targets() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("speed");
    fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let qr_png = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/qr.png");

    let mut big = photo(8000, 6000, 8);
    imageops::overlay(&mut big, &qr("48 megapixels", 2), 5200, 3900);
    fs::write(file("48mp.jpg"), jpeg(&big)).unwrap();
    fs::write(file("12mp.jpg"), jpeg(&photo(4000, 3000, 8))).unwrap();
    let page = (595.0, 842.0, vector_qr("a4 page", 72.0, 72.0, 4.0, 842.0));
    fs::write(file("a4.pdf"), pdf_file(&[page], "")).unwrap();
    let scans: Vec<_> = (1..=20)
        .map(|n| {
            let mut paper = GrayImage::from_pixel(2480, 3508, Luma([245]));
            imageops::overlay(
                &mut paper,
                &qr(&format!("page {n}"), 2),
                1800,
                300 + 140 * n,
            );
            jpeg(&paper)
        })
        .collect();
    fs::write(file("scans.pdf"), scanned_pdf(&scans)).unwrap();

    let (version_ms, png_ms) = if cfg!(windows) { (30, 40) } else { (10, 15) };
    let ms = Duration::from_millis;
    let version = || {
        assert!(
            Command::new(BIN)
                .arg("--version")
                .output()
                .unwrap()
                .status
                .success()
        )
    };
    let mut rows = vec![
        ("--version", median(version), ms(version_ms)),
        ("132 px QR PNG", median(|| cli(&[qr_png], 1)), ms(png_ms)),
        (
            "48 MP JPEG",
            median(|| cli(&[&file("48mp.jpg")], 1)),
            ms(1500),
        ),
        (
            "12 MP photo, no code",
            median(|| cli(&[&file("12mp.jpg")], 0)),
            ms(1500),
        ),
        (
            "A4 vector PDF page",
            median(|| cli(&[&file("a4.pdf")], 1)),
            ms(400),
        ),
        (
            "20 scanned A4 pages",
            median(|| cli(&[&file("scans.pdf")], 20)),
            ms(8000),
        ),
    ];

    let mut ui = Command::new(BIN)
        .args(["ui", "--no-open"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(ui.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = &line["Open http://127.0.0.1:".len()..line.find("  (").unwrap()];
    let body = fs::read(qr_png).unwrap();
    let post = || {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        let head = format!(
            "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-File-Name: qr.png\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 200") && answer.contains("barcode-read smoke test"));
    };
    rows.push(("page: POST /api/read", median(post), ms(60)));
    ui.kill().unwrap();
    ui.wait().unwrap();

    let messages = format!(
        "{}\n{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"speed","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "read_barcodes", "arguments": {"paths": [qr_png]}}}),
    );
    let mcp = || {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(messages.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("barcode-read smoke test"));
    };
    rows.push(("mcp: initialize, list, call", median(mcp), ms(150)));

    for (case, time, target) in &rows {
        let ms = time.as_secs_f64() * 1e3;
        eprintln!(
            "{case:<30} {ms:>8.1} ms  (target {} ms)",
            target.as_millis()
        );
    }
    let size = fs::metadata(BIN).unwrap().len();
    eprintln!(
        "{:<30} {:>8.1} MB  (target 14 MB)",
        "binary",
        size as f64 / 1e6
    );
    assert!(size <= 14_000_000, "binary is {size} bytes");
    let slow: Vec<_> = rows
        .iter()
        .filter(|(_, time, target)| time > target)
        .collect();
    assert!(slow.is_empty(), "slower than the target: {slow:?}");
}

#[test]
#[ignore = "known bug: QA-06"]
fn reads_a_noisy_48_mp_photo_in_time() {
    let mut big = photo(8000, 6000, 24);
    imageops::overlay(&mut big, &qr("noisy", 2), 5200, 3900);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("noisy-48mp.jpg");
    fs::write(&path, jpeg(&big)).unwrap();
    let start = Instant::now();
    cli(&[path.to_str().unwrap()], 1);
    assert!(
        start.elapsed() <= Duration::from_millis(1500),
        "{:?}",
        start.elapsed()
    );
}
