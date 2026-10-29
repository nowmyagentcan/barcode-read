//! The binary end to end: inputs, every file and usage error, exit codes, JSON and --debug.

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{bytes_code, code, encode, pdf_file};
use image::{GrayImage, ImageFormat, Luma};
use serde_json::{Value, json};
use zxingcpp::BarcodeFormat;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).expect(&self.stdout)
    }
}

/// Runs the binary in `dir` with `args`, feeding it `stdin` (empty when `None`).
fn run(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_barcode-read"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.unwrap_or_default()).unwrap();
    drop(input);
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// A fresh, empty folder for one test.
fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn qr_png(text: &str) -> Vec<u8> {
    encode(code(BarcodeFormat::QRCode, text, ""), ImageFormat::Png)
}

fn blank_png() -> Vec<u8> {
    encode(GrayImage::from_pixel(40, 40, Luma([255])), ImageFormat::Png)
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn prints_the_version_and_a_help_with_formats_and_examples() {
    let dir = folder("help");
    for flag in ["--version", "-V"] {
        let run = run(&dir, &[flag], None);
        assert_eq!((run.code, run.stdout.as_str()), (0, "barcode-read 0.1.0\n"));
    }
    let help = run(&dir, &["--help"], None);
    assert_eq!(help.code, 0);
    for part in [
        "Read QR codes and barcodes from photos, scans and PDFs.",
        "--formats <LIST>",
        "--debug",
        "QR Code, ",
        "Data Matrix",
        "Groups: all-linear, all-matrix, all-gs1, all-retail, all-industrial",
        "  cat code.png | barcode-read -\n",
    ] {
        assert!(help.stdout.contains(part), "{part}");
    }
    let formats = help.stdout.split("Formats").nth(1).unwrap();
    assert!(formats.lines().all(|l| l.chars().count() <= 80));
}

/// One case of spec §8.1: a file, extra arguments, and the error it must give. A message
/// ending in ": " is a prefix, for messages that carry an OS or decoder text.
struct Case {
    name: &'static str,
    bytes: Option<Vec<u8>>,
    args: &'static [&'static str],
    code: &'static str,
    message: String,
    hint: Option<&'static str>,
}

fn case(name: &'static str, bytes: Option<Vec<u8>>, code: &'static str, message: &str) -> Case {
    Case {
        name,
        bytes,
        args: &[],
        code,
        message: message.into(),
        hint: None,
    }
}

impl Case {
    fn hint(self, hint: &'static str) -> Self {
        Self {
            hint: Some(hint),
            ..self
        }
    }
    fn args(self, args: &'static [&'static str]) -> Self {
        Self { args, ..self }
    }
}

const SUPPORTED: &str = "Use PNG, JPEG, GIF, BMP, WebP, TIFF, PNM or PDF.";

#[test]
fn every_file_error_has_its_message_and_hint_in_text_and_json() {
    let dir = folder("errors");
    let mut bmp = b"BM".to_vec();
    bmp.extend([0; 12].iter().chain(&40u32.to_le_bytes()));
    bmp.extend(70_000i32.to_le_bytes().iter().chain(&1i32.to_le_bytes()));
    let encrypt = "/Encrypt << /Filter /Adobe.PubSec /V 4 /R 4 >> /ID [<00> <00>]";
    let pubsec = pdf_file(&[(595.0, 842.0, String::new())], encrypt);
    let heic = "Export it as JPEG first (iPhone: Settings > Camera > Formats > Most Compatible).";
    let cases = [
        case(
            "missing.png",
            None,
            "not_found",
            "File not found: missing.png",
        )
        .hint("Check the path."),
        case("*.png", None, "wildcard", "No file named '*.png'.").hint(
            "Windows shells don't expand * or ?; pass the folder instead, e.g. barcode-read photos",
        ),
        case(
            "empty.png",
            Some(vec![]),
            "empty_file",
            "The file is empty.",
        ),
        case(
            "photo.heic",
            Some(b"\0\0\0\x18ftypheic\0\0\0\0".to_vec()),
            "unsupported_format",
            "HEIC/HEIF photos aren't supported yet.",
        )
        .hint(heic),
        case(
            "photo.avif",
            Some(b"\0\0\0\x1Cftypavif\0\0\0\0".to_vec()),
            "unsupported_format",
            "AVIF images aren't supported yet.",
        )
        .hint(SUPPORTED),
        case(
            "photo.jxl",
            Some(b"\xFF\x0A\xFA\x7F".to_vec()),
            "unsupported_format",
            "JPEG XL images aren't supported yet.",
        )
        .hint(SUPPORTED),
        case(
            "photo.jp2",
            Some(b"\xFF\x4F\xFF\x51\0\x2F".to_vec()),
            "unsupported_format",
            "JPEG 2000 images aren't supported yet.",
        )
        .hint(SUPPORTED),
        case(
            "logo.svg",
            Some(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec()),
            "unsupported_format",
            "SVG files aren't supported.",
        )
        .hint("Render it to PNG first."),
        case(
            "notes.png",
            Some(b"hello".to_vec()),
            "unsupported_format",
            "This isn't a supported image or PDF.",
        )
        .hint(SUPPORTED),
        case(
            "wide.bmp",
            Some(bmp),
            "image_too_large",
            "The image is 70000x1 pixels; the limit is 120 megapixels and 65,535 pixels per side.",
        )
        .hint("Crop or resize it first."),
        case(
            "cut.png",
            Some(qr_png("cut")[..40].to_vec()),
            "corrupt_image",
            "Can't decode this PNG file: ",
        )
        .hint("The file may be damaged or cut short."),
        case(
            "bad.pdf",
            Some(b"%PDF-1.7\nnot really".to_vec()),
            "corrupt_pdf",
            "Can't open this PDF: its structure can't be read.",
        )
        .hint("The file may be damaged."),
        case(
            "aes.pdf",
            Some(fixture("aes.pdf")),
            "pdf_password",
            "This PDF needs a password.",
        )
        .hint("Pass it with --password."),
        case(
            "aes.pdf",
            None,
            "pdf_wrong_password",
            "The PDF password is wrong.",
        )
        .args(&["--password", "wrong"]),
        case(
            "pubsec.pdf",
            Some(pubsec),
            "pdf_encryption",
            "This PDF uses an encryption method that isn't supported.",
        ),
        case(
            "scan.pdf",
            Some(fixture("scan.pdf")),
            "no_pages",
            "--pages 9 selects none of this file's 5 pages.",
        )
        .args(&["--pages", "9"]),
        unreadable(&dir),
    ];
    for c in cases {
        if let Some(bytes) = &c.bytes {
            fs::write(dir.join(c.name), bytes).unwrap();
        }
        let args = [&[c.name], c.args].concat();
        let text = run(&dir, &args, None);
        assert_eq!((text.code, text.stdout.as_str()), (1, ""), "{}", c.code);
        let mut lines = text.stderr.lines();
        let first = lines.next().unwrap();
        let message = first.strip_prefix(&format!("✗ {}: ", c.name)).unwrap();
        assert!(matches(message, &c.message), "{first}");
        assert_eq!(lines.next(), c.hint.map(|h| format!("  {h}")).as_deref());
        assert_eq!(lines.next(), None);

        let json = run(&dir, &[&args[..], &["--json"]].concat(), None);
        assert_eq!(json.code, 1, "{}", c.code);
        let report = json.json();
        let file = &report["files"][0];
        assert_eq!(file["path"], c.name);
        assert_eq!(
            (&file["barcodes"], &report["failed"]),
            (&json!([]), &json!(1))
        );
        let e = &file["error"];
        assert_eq!((&e["code"], &e["hint"]), (&json!(c.code), &json!(c.hint)));
        assert!(matches(e["message"].as_str().unwrap(), &c.message));
    }
}

fn matches(message: &str, expected: &str) -> bool {
    match expected.strip_suffix(": ") {
        Some(prefix) => message.starts_with(&format!("{prefix}: ")) && message.ends_with('.'),
        None => message == expected,
    }
}

/// A file the OS won't let us read: on Windows one held open without sharing, elsewhere a
/// path through a file as if it were a folder.
fn unreadable(dir: &Path) -> Case {
    let error = case("", None, "unreadable", "");
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = fs::File::create_new(dir.join("locked.png")).unwrap();
        drop(file);
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.join("locked.png"))
            .unwrap();
        std::mem::forget(held);
        Case {
            name: "locked.png",
            message: "Can't read locked.png: ".into(),
            ..error
        }
    }
    #[cfg(not(windows))]
    {
        fs::write(dir.join("plain.png"), b"").unwrap();
        Case {
            name: "plain.png/inner.png",
            message: "Can't read plain.png/inner.png: ".into(),
            ..error
        }
    }
}

#[test]
fn a_file_over_1_gib_is_refused_from_its_size() {
    let dir = folder("large");
    let file = fs::File::create(dir.join("huge.png")).unwrap();
    file.set_len((1 << 30) + 1).unwrap();
    drop(file);
    let run = run(&dir, &["huge.png"], None);
    assert_eq!(run.code, 1);
    assert_eq!(
        run.stderr,
        "✗ huge.png: The file is larger than 1 GiB.\n  Split the PDF or shrink the image first.\n"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn usage_errors_exit_2_as_two_lines_or_a_json_object() {
    let dir = folder("usage");
    let no_input = (
        "no_input",
        "No file given.",
        Some("Try: barcode-read photo.jpg"),
    );
    let formats =
        Some("Use names like qrcode, datamatrix, ean13, code128, all-linear or all-matrix.");
    let pages = Some("Use page numbers and ranges like 1-3,7,10-");
    let dpi = ("bad_dpi", "--dpi must be between 72 and 1200.", None);
    let cases: [(&[&str], _); 7] = [
        (&[], no_input),
        (&["read"], no_input),
        (
            &["a.png", "--formats", "qr"],
            ("unknown_format", "Unknown barcode format 'qr'.", formats),
        ),
        (
            &["--pages", "3-1", "a.png"],
            ("bad_pages", "Can't read the page list '3-1'.", pages),
        ),
        (&["a.png", "--dpi", "50"], dpi),
        (&["a.png", "--dpi", "1201"], dpi),
        (&["a.png", "--dpi=abc"], dpi),
    ];
    for (args, (code, message, hint)) in cases {
        let text = run(&dir, args, None);
        let expected = match hint {
            Some(h) => format!("✗ {message}\n  {h}\n"),
            None => format!("✗ {message}\n"),
        };
        assert_eq!((text.code, text.stdout.as_str()), (2, ""), "{args:?}");
        assert_eq!(text.stderr, expected);
        let json = run(&dir, &[args, &["--json"]].concat(), None);
        assert_eq!(json.code, 2);
        assert_eq!(
            json.json(),
            json!({"error": {"code": code, "message": message, "hint": hint}})
        );
        assert_eq!(json.stderr, "");
    }

    let text = run(&dir, &["a.png", "--nope"], None);
    assert_eq!((text.code, text.stdout.as_str()), (2, ""));
    assert!(text.stderr.contains("'--nope'") && text.stderr.contains("--help"));
    let json = run(&dir, &["--json", "a.png", "--nope"], None);
    assert_eq!(json.code, 2);
    assert_eq!(
        json.json(),
        json!({"error": {
            "code": "usage",
            "message": "Unexpected argument '--nope' found.",
            "hint": "Run barcode-read --help to see the options."
        }})
    );
}

/// The `path` of each file in a JSON run.
fn paths(run: &Run) -> Vec<String> {
    let files = run.json()["files"].as_array().unwrap().clone();
    files
        .iter()
        .map(|f| f["path"].as_str().unwrap().replace('\\', "/"))
        .collect()
}

#[test]
fn folders_expand_in_name_order_and_dash_reads_stdin() {
    let dir = folder("inputs");
    fs::create_dir_all(dir.join("codes/sub")).unwrap();
    for (name, text) in [
        ("codes/b.png", "b"),
        ("codes/A.PNG", "A"),
        ("codes/sub/c.png", "c"),
    ] {
        fs::write(dir.join(name), qr_png(text)).unwrap();
    }
    fs::write(dir.join("codes/.hidden.png"), qr_png("hidden")).unwrap();
    fs::write(dir.join("codes/notes.txt"), qr_png("notes")).unwrap();

    let flat = run(&dir, &["codes", "--json"], None);
    assert_eq!(paths(&flat), ["codes/A.PNG", "codes/b.png"]);
    assert_eq!(flat.code, 0);
    let deep = run(&dir, &["-r", "codes"], None);
    let heads: Vec<_> = deep
        .stdout
        .lines()
        .filter(|l| !l.starts_with("  "))
        .collect();
    assert_eq!(heads.len(), 4, "{}", deep.stdout);
    assert!(
        heads[2]
            .replace('\\', "/")
            .starts_with("codes/sub/c.png  QR Code")
    );
    assert_eq!(heads[3], "3 codes in 3 of 3 files");

    // Options before or after paths, `read` only as the first argument, `--` ends options.
    let png = qr_png("from stdin");
    let mixed = run(
        &dir,
        &[
            "read",
            "--json",
            "-",
            "codes/b.png",
            "--fast",
            "codes/b.png",
            "--",
            "read",
        ],
        Some(&png),
    );
    assert_eq!(paths(&mixed), ["-", "codes/b.png", "codes/b.png", "read"]);
    let report = mixed.json();
    assert_eq!(report["files"][0]["barcodes"][0]["text"], "from stdin");
    assert_eq!(report["files"][3]["error"]["code"], "not_found");
    assert_eq!(
        (&report["found"], &report["failed"]),
        (&json!(3), &json!(1))
    );
    assert_eq!(mixed.code, 1);
}

#[test]
fn exit_codes_follow_what_was_found_and_what_failed() {
    let dir = folder("exit");
    fs::write(dir.join("qr.png"), qr_png("found")).unwrap();
    fs::write(dir.join("blank.png"), blank_png()).unwrap();

    let found = run(&dir, &["qr.png"], None);
    assert_eq!(
        (found.code, found.stdout.as_str()),
        (0, "qr.png  QR Code\n  found\n")
    );
    let none = run(&dir, &["blank.png"], None);
    assert_eq!(none.code, 1);
    assert_eq!(
        none.stdout,
        "blank.png  no barcode found\n  Try a sharper, closer photo with the whole code in view.\n"
    );
    let some = run(&dir, &["blank.png", "qr.png"], None);
    assert_eq!(some.code, 0);
    assert!(some.stdout.ends_with("1 code in 1 of 2 files\n"));
    let failed = run(&dir, &["qr.png", "missing.png"], None);
    assert_eq!(failed.code, 1);
    assert!(
        failed
            .stdout
            .ends_with("1 code in 1 of 2 files · 1 failed\n")
    );
}

#[test]
fn text_output_has_tags_pages_warnings_and_indented_lines() {
    let dir = folder("text");
    let vcard = "BEGIN:VCARD\nVERSION:3.0\nFN:Mei Lee\nEND:VCARD";
    let gs1 = code(BarcodeFormat::Code128, "(01)09501101530003(10)LOT7", "gs1");
    let bytes: Vec<u8> = (0..=255).collect();
    let files = [
        ("vcard.png", qr_png(vcard)),
        ("gs1.png", encode(gs1, ImageFormat::Png)),
        (
            "bytes.png",
            encode(bytes_code(BarcodeFormat::QRCode, &bytes), ImageFormat::Png),
        ),
        ("scan.pdf", fixture("scan.pdf")),
    ];
    for (name, bytes) in &files {
        fs::write(dir.join(name), bytes).unwrap();
    }
    let run = run(
        &dir,
        &[
            "vcard.png",
            "gs1.png",
            "bytes.png",
            "scan.pdf",
            "--pages",
            "5-6",
        ],
        None,
    );
    assert_eq!(run.code, 0);
    let lines: Vec<_> = run.stdout.lines().collect();
    assert_eq!(
        lines[..8],
        [
            "vcard.png  QR Code",
            "  BEGIN:VCARD",
            "  VERSION:3.0",
            "  FN:Mei Lee",
            "  END:VCARD",
            "gs1.png  Code 128  GS1",
            "  (01)09501101530003(10)LOT7",
            "bytes.png  QR Code  binary, 256 bytes",
        ]
    );
    assert!(lines[8].starts_with("  <NUL><SOH><STX>") && lines[8].contains("~<DEL><U+80>"));
    assert_eq!(
        lines[lines.len() - 3..],
        [
            "scan.pdf  page 5  QR Code",
            "  JPXDecode",
            "4 codes in 4 of 4 files"
        ]
    );
    assert_eq!(
        run.stderr,
        "! scan.pdf: This file has 5 pages; page 6 doesn't exist.\n"
    );
    assert!(!run.stdout.contains('\x1b') && !run.stderr.contains('\x1b'));
}

#[test]
fn control_and_bidi_characters_in_payloads_never_reach_the_terminal() {
    let dir = folder("escape");
    let payloads = [
        ("esc.png", "hi\x1b[31mRED\x1b[0m", "hi<ESC>[31mRED<ESC>[0m"),
        ("cr.png", "one\rtwo", "one<CR>two"),
        ("c1.png", "x\u{9b}31m", "x<U+009B>31m"),
        (
            "rlo.png",
            "invoice\u{202e}fdp.exe",
            "invoice<U+202E>fdp.exe",
        ),
    ];
    let mut args = vec![];
    for (name, text, _) in payloads {
        fs::write(dir.join(name), qr_png(text)).unwrap();
        args.push(name);
    }
    args.push("missing\u{202e}gnp.png");
    let run = run(&dir, &args, None);
    for (_, _, shown) in payloads {
        assert!(run.stdout.contains(&format!("\n  {shown}\n")), "{shown}");
    }
    assert!(
        run.stderr
            .starts_with("✗ missing<U+202E>gnp.png: File not found: missing<U+202E>gnp.png\n")
    );
    // `run` already failed on any output that isn't UTF-8, so checking characters covers every
    // byte.
    for c in run.stdout.chars().chain(run.stderr.chars()) {
        assert!(!c.is_control() || c == '\n', "{c:?}");
        assert!(
            !matches!(c, '\u{61C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        );
    }
}

#[test]
fn json_pages_carry_dpi_only_for_pdfs() {
    let dir = folder("json");
    fs::write(dir.join("qr.png"), qr_png("png")).unwrap();
    fs::write(dir.join("scan.pdf"), fixture("scan.pdf")).unwrap();
    let run = run(
        &dir,
        &["qr.png", "scan.pdf", "--pages", "2", "--json"],
        None,
    );
    assert_eq!(run.code, 0);
    assert!(run.stdout.ends_with("}\n") && run.stdout.lines().count() == 1);
    let report = run.json();
    let (png, pdf) = (&report["files"][0], &report["files"][1]);
    assert_eq!((&png["type"], &pdf["type"]), (&json!("png"), &json!("pdf")));
    assert!(png["pages"][0].get("dpi").is_none());
    assert_eq!(
        pdf["pages"],
        json!([{"page": 2, "width": 750, "height": 750, "dpi": 300}])
    );
    assert_eq!(pdf["barcodes"][0]["page"], 2);
    assert_eq!(pdf["barcodes"][0]["text"], "FlateDecode");
    // Keys in the documented order.
    let keys = [
        "\"path\"",
        "\"type\"",
        "\"pages\"",
        "\"barcodes\"",
        "\"warnings\"",
        "\"error\"",
    ];
    let at: Vec<_> = keys.iter().map(|k| run.stdout.find(k).unwrap()).collect();
    assert!(at.is_sorted(), "{}", run.stdout);
    assert!(run.stdout.ends_with("\"found\":2,\"failed\":0}\n"));
}

#[test]
fn a_closed_stdout_or_stderr_ends_the_run_quietly_with_0() {
    let dir = folder("pipes");
    fs::write(dir.join("qr.png"), qr_png("closed")).unwrap();
    let cases: [(&[&str], bool); 6] = [
        (&["qr.png", "qr.png"], true),
        (&["qr.png", "--json"], true),
        (&["qr.png", "--debug"], true),
        (&["--help"], true),
        (&["qr.png", "--debug"], false),
        (&["missing.png", "qr.png"], false),
    ];
    for (args, close_stdout) in cases {
        let mut child = Command::new(env!("CARGO_BIN_EXE_barcode-read"))
            .args(args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if close_stdout {
            drop(child.stdout.take());
        } else {
            drop(child.stderr.take());
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let other = if close_stdout { out.stderr } else { out.stdout };
        assert!(!String::from_utf8(other).unwrap().contains("Can't write"));
    }
}

/// Interactive mode in a real terminal, through util-linux `script`, which gives the binary a
/// pseudo-terminal and types `typed` into it.
#[cfg(target_os = "linux")]
fn terminal(dir: &Path, typed: &str, no_color: bool) -> String {
    let mut script = Command::new("script");
    script
        .args(["-qec", env!("CARGO_BIN_EXE_barcode-read"), "/dev/null"])
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if no_color {
        script.env("NO_COLOR", "1");
    }
    let mut child = script.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(typed.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().replace("\r\n", "\n")
}

#[cfg(target_os = "linux")]
#[test]
fn interactive_mode_reads_dropped_paths_in_a_terminal() {
    let dir = folder("interactive");
    fs::write(dir.join("boarding pass.png"), qr_png("M1LEE/MEI")).unwrap();
    let typed = "'boarding pass.png'\n/help\n/quit\nboarding\\ pass.png\n";
    let plain = terminal(&dir, typed, true);
    assert!(!plain.contains('\x1b'), "{plain}");
    for part in [
        "\n╭─────",
        "│ ✻ barcode-read 0.1.0 ",
        "│   Read QR codes and barcodes from images and PDFs. │\n",
        "\n  Drop files here or type paths · /help · /quit\n\n› ",
        "  boarding pass.png  QR Code\n    M1LEE/MEI\n\n› ",
        "  /quit   leave (Ctrl+C works too)\n\n› ",
    ] {
        assert!(plain.contains(part), "{part}\n{plain}");
    }
    // Lines after /quit aren't read.
    assert_eq!(plain.matches("M1LEE/MEI").count(), 1, "{plain}");
    let colour = terminal(&dir, "boarding\\ pass.png\n", false);
    assert!(
        colour.contains("\x1b[1m✻ barcode-read\x1b[0m 0.1.0"),
        "{colour}"
    );
    assert!(colour.contains("  \x1b[1mboarding pass.png\x1b[0m  QR Code\n"));

    // Without a terminal the same start is a usage error.
    let piped = run(&dir, &[], Some(b"boarding\\ pass.png\n"));
    assert_eq!((piped.code, piped.stdout.as_str()), (2, ""));
    assert!(piped.stderr.starts_with("✗ No file given.\n"));
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8_prints_lossily() {
    use std::os::unix::ffi::OsStrExt;
    let dir = folder("lossy");
    let name = std::ffi::OsStr::from_bytes(b"caf\xE9.png");
    let out = Command::new(env!("CARGO_BIN_EXE_barcode-read"))
        .arg(name)
        .arg("--json")
        .current_dir(&dir)
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["files"][0]["path"], "caf\u{FFFD}.png");
    assert_eq!(report["files"][0]["error"]["code"], "not_found");
}

#[test]
fn debug_shows_each_pass_on_stderr_only() {
    let dir = folder("debug");
    fs::write(dir.join("blank.png"), blank_png()).unwrap();
    let plain = run(&dir, &["blank.png", "--json"], None);
    let debug = run(&dir, &["blank.png", "--json", "--debug"], None);
    assert_eq!(debug.stdout, plain.stdout);
    assert_eq!(plain.stderr, "");
    assert!(debug.stderr.starts_with("debug: blank.png: png, read in "));
    assert!(debug.stderr.contains("page 1 40x40: pass 1 "));
    assert!(debug.stderr.contains(", pass 3 ") && !debug.stderr.contains("pass 4"));
    let fast = run(&dir, &["blank.png", "--fast", "--debug"], None);
    assert!(fast.stderr.contains("pass 1 ") && !fast.stderr.contains("pass 2"));
    let missing = run(&dir, &["gone.png", "--debug"], None);
    assert!(
        missing
            .stderr
            .contains("debug: gone.png: error not_found\n")
    );
}

#[test]
fn a_deep_folder_tree_ends_with_a_report() {
    let dir = folder("deep");
    let mut deep = dir.join("tree");
    // macOS refuses paths over 1024 bytes; Windows overflowed its stack at 600 levels.
    let levels = if cfg!(target_os = "macos") { 400 } else { 1000 };
    for _ in 0..levels {
        deep.push("d");
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("qr.png"), qr_png("deep down")).unwrap();
    let run = run(&dir, &["tree", "-r", "--json"], None);
    assert!(run.code <= 1, "exit {}: {}", run.code, run.stderr);
    assert_eq!(run.json()["files"].as_array().unwrap().len(), 1);
}
