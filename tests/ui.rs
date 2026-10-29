//! The local page: the HTML itself, its text and CSV builders, and `barcode-read ui` answering
//! over HTTP on 127.0.0.1.

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use common::{code, encode};
use image::{GrayImage, ImageFormat, Luma};
use serde_json::{Value, json};
use zxingcpp::BarcodeFormat;

const PAGE: &str = include_str!("../src/ui.html");
const BIN: &str = env!("CARGO_BIN_EXE_barcode-read");

#[test]
fn page_loads_nothing_from_another_host() {
    let footer = [
        "https://nowmyagentcan.com",
        "https://github.com/nowmyagentcan/barcode-read",
        "https://x.com/chayp05",
    ];
    for pat in ["src=", "href=", "url(", "@import", "fetch(", "action="] {
        for (i, _) in PAGE.match_indices(pat) {
            let rest = PAGE[i + pat.len()..].trim_start_matches(['"', '\'', ' ']);
            let target = &rest[..rest.find(['"', '\'', ' ', ')', '>']).unwrap_or(rest.len())];
            assert!(
                target.starts_with("data:") || !target.contains("://") || footer.contains(&target),
                "{pat} loads {target}"
            );
        }
    }
    assert!(PAGE.contains(r#"fetch("/api/read""#));
}

/// A folder of inputs for one test: a QR, a GS1 Code 128 and a page with no code.
fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let qr = code(BarcodeFormat::QRCode, "hello page", "");
    let gs1 = code(BarcodeFormat::Code128, "(01)09501101530003(10)LOT7", "gs1");
    let blank = GrayImage::from_pixel(40, 40, Luma([255]));
    for (name, img) in [("qr.png", qr), ("gs1.png", gs1), ("blank.png", blank)] {
        fs::write(dir.join(name), encode(img, ImageFormat::Png)).unwrap();
    }
    dir
}

/// The CLI's `--json` output for `args`, run in `dir`.
fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new(BIN)
        .args(args)
        .arg("--json")
        .current_dir(dir)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The page's own text and CSV builders, run in Node.js on `files`.
fn page_builders(files: &Value) -> Value {
    let start = PAGE.find("// Payload text is untrusted").unwrap();
    let end = PAGE.find("function failure(").unwrap();
    let script = format!(
        "{}\nconst files = JSON.parse(require('fs').readFileSync(0, 'utf8'));\n\
         console.log(JSON.stringify({{ text: asText(files), csv: csv(files), summary: summary(files),\n\
         shown: shown('a\\x1b[31m\\r\\u009b\\u202e\\u0007\\x7f\\tz') }}));",
        &PAGE[start..end]
    );
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("page-builders.js");
    fs::write(&path, script).unwrap();
    let mut node = Command::new("node")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the page's script runs in Node.js, which must be on PATH");
    node.stdin
        .take()
        .unwrap()
        .write_all(files.to_string().as_bytes())
        .unwrap();
    let out = node.wait_with_output().unwrap();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn page_copies_the_cli_text_and_guards_the_csv() {
    let dir = folder("builders");
    let names = ["qr.png", "gs1.png", "blank.png"];
    let mut files = cli(&dir, &names)["files"].clone();
    let text = Command::new(BIN)
        .args(names)
        .current_dir(&dir)
        .output()
        .unwrap()
        .stdout;
    // A payload a spreadsheet would run as a formula, in a file whose name would be one too.
    files[0]["path"] = json!("=x.png");
    files[0]["barcodes"][0]["text"] = json!("=HYPERLINK(\"x\")");
    let page = page_builders(&files);
    let cli_text = String::from_utf8(text).unwrap();
    assert_eq!(
        page["text"].as_str().unwrap().replace("=x.png", "qr.png"),
        cli_text.replace("hello page", "=HYPERLINK(\"x\")")
    );
    assert_eq!(page["summary"], "2 codes in 2 of 3 files");
    assert_eq!(
        page["csv"],
        "\u{feff}\"file\",\"page\",\"format\",\"content\",\"text\"\r\n\
         \"'=x.png\",\"1\",\"QR Code\",\"text\",\"'=HYPERLINK(\"\"x\"\")\"\r\n\
         \"gs1.png\",\"1\",\"Code 128\",\"gs1\",\"(01)09501101530003(10)LOT7\"\r\n"
    );
    assert_eq!(page["shown"], "a<ESC>[31m<CR><U+009B><U+202E><BEL><DEL>\tz");
}

/// What `tests/page.js` saw the page's own script do: a slow run overtaken, then refused DPIs.
fn page_runs() -> Value {
    let out = Command::new("node")
        .args(["tests/page.js", "src/ui.html"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("the page's script runs in Node.js, which must be on PATH");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

const BAD_DPI: &str = "DPI must be a whole number from 72 to 1200.";

#[test]
fn page_ends_each_run_cleanly_and_shows_a_bad_dpi() {
    let page = page_runs();
    // A slow first file must not leave the newer run marked busy.
    assert_eq!(page["busy"], Value::Null);
    // Options is closed, so the field itself can't show why: the result says it.
    assert_eq!(
        page["dpi"],
        json!({ "sent": [], "open": true, "shown": BAD_DPI, "focused": true })
    );
}

#[test]
fn page_refuses_a_dpi_of_zero() {
    assert_eq!(
        page_runs()["zero"],
        json!({ "sent": [], "open": true, "shown": BAD_DPI, "focused": true })
    );
}

/// `barcode-read ui` on a free port, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Server {
    fn start(args: &[&str]) -> Server {
        let mut child = Command::new(BIN)
            .arg("ui")
            .args(args)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port = line
            .strip_prefix("Open http://127.0.0.1:")
            .and_then(|rest| rest.strip_suffix("  (Ctrl+C to stop)\n"))
            .unwrap_or_else(|| panic!("{line:?}"))
            .parse()
            .unwrap();
        Server { child, port }
    }

    /// Sends `head` (request line and headers, `{port}` filled in) and `body`; returns the
    /// status, the headers and the body.
    fn send(&self, head: &str, body: &[u8]) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = head.replace("{port}", &self.port.to_string());
        stream
            .write_all(format!("{head}\r\n\r\n").as_bytes())
            .unwrap();
        stream.write_all(body).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let status = headers[9..12].parse().unwrap();
        (status, headers.to_owned(), body.to_owned())
    }

    /// Uploads `body` as the page does.
    fn read(&self, name: &str, options: &str, body: &[u8]) -> (u16, Value) {
        let head = format!(
            "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nOrigin: http://127.0.0.1:{{port}}\r\n\
             X-File-Name: {name}\r\nX-Options: {options}\r\nContent-Length: {}",
            body.len()
        );
        let (status, _, body) = self.send(&head, body);
        (status, serde_json::from_str(&body).unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

#[test]
fn serves_the_page_and_reads_like_the_cli() {
    let dir = folder("reads");
    let server = Server::start(&["--no-open"]);
    let (status, headers, body) = server.send("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}", b"");
    assert_eq!((status, body.as_str()), (200, PAGE));
    assert!(headers.contains("Content-Type: text/html; charset=utf-8\r\n"));
    assert!(headers.contains("Content-Security-Policy: default-src 'none';"));
    let qr = fs::read(dir.join("qr.png")).unwrap();
    for (options, args) in [
        ("{}", &[][..]),
        (
            r#"{"formats":"all-matrix","dpi":150,"fast":true}"#,
            &["--formats", "all-matrix", "--dpi", "150", "--fast"],
        ),
        (r#"{"formats":"ean13"}"#, &["--formats", "ean13"]),
    ] {
        let want = &cli(&dir, &[&["qr.png"][..], args].concat())["files"][0];
        assert_eq!(server.read("qr.png", options, &qr), (200, want.clone()));
    }
    let (status, file) = server.read("Gr%C3%BC%C3%9Fe%20%E6%9D%B1.png", "{}", &qr);
    assert_eq!((status, &file["path"]), (200, &json!("Grüße 東.png")));
    let (status, file) = server.read("x.heic", "{}", b"\0\0\0\x18ftypheic\0\0\0\0");
    assert_eq!(
        (status, &file["error"]["code"]),
        (200, &json!("unsupported_format"))
    );
    let (status, file) = server.read("empty.png", "{}", b"");
    assert_eq!(
        (status, &file["error"]["code"]),
        (200, &json!("empty_file"))
    );
    // A password past ASCII arrives as the page sends it: JSON with \u escapes.
    let aes = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aes.pdf"));
    let (status, file) = server.read("aes.pdf", r#"{"password":"päss"}"#, &aes.unwrap());
    assert_eq!(
        (status, &file["error"]["code"]),
        (200, &json!("pdf_wrong_password"))
    );
    for (options, args) in [
        (r#"{"formats":"qr"}"#, &["--formats", "qr"][..]),
        (r#"{"pages":"3-1"}"#, &["--pages", "3-1"]),
        (r#"{"dpi":50}"#, &["--dpi", "50"]),
    ] {
        let want = cli(&dir, &[&["qr.png"][..], args].concat());
        assert_eq!(
            server.read("qr.png", options, &qr),
            (422, want),
            "{options}"
        );
    }
    for (name, options) in [
        ("qr.png", "{nope"),
        ("qr.png", r#"{"fats":true}"#),
        ("qr.png", r#"{"dpi":-1}"#),
        ("%zz.png", "{}"),
        ("%C3.png", "{}"),
    ] {
        let (status, error) = server.read(name, options, &qr);
        assert_eq!(
            (status, &error["error"]["code"]),
            (400, &json!("bad_request")),
            "{name} {options}"
        );
    }
}

#[test]
fn refuses_other_hosts_origins_routes_and_large_requests() {
    let server = Server::start(&["--no-open"]);
    let other = server.port.wrapping_add(1);
    for (head, status) in [
        ("GET / HTTP/1.1\r\nHost: localhost:{port}".to_owned(), 200),
        ("GET /?x=1 HTTP/1.1\r\nHost: LOCALHOST:{port}".into(), 200),
        ("GET / HTTP/1.1".into(), 403),
        ("GET / HTTP/1.1\r\nHost: evil.example".into(), 403),
        ("GET / HTTP/1.1\r\nHost: evil.example:{port}".into(), 403),
        (format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{other}"), 403),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example".into(),
            403,
        ),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://127.0.0.1:{port}".into(),
            403,
        ),
        (
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: null".into(),
            403,
        ),
        (
            "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://evil.example\r\nContent-Length: 0".into(),
            403,
        ),
        ("GET /api/read HTTP/1.1\r\nHost: 127.0.0.1:{port}".into(), 404),
        ("GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1:{port}".into(), 404),
        ("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0".into(), 404),
        ("POST /api HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0".into(), 404),
    ] {
        assert_eq!(server.send(&head, b"").0, status, "{head}");
    }
    // Refused from the length alone, before any of the body is read.
    let head = format!(
        "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nContent-Length: {}",
        (256u64 << 20) + 1
    );
    let (status, _, body) = server.send(&head, b"");
    assert_eq!(
        (status, body.as_str()),
        (
            413,
            r#"{"error":{"code":"too_large","message":"The file is larger than 256 MiB.","hint":null}}"#
        )
    );
    let head = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{{port}}\r\nX-Pad: {}",
        "a".repeat(17 * 1024)
    );
    assert_eq!(server.send(&head, b"").0, 413);
}

#[test]
fn refuses_repeated_headers_framing_and_malformed_requests() {
    let dir = folder("malformed");
    let qr = fs::read(dir.join("qr.png")).unwrap();
    let server = Server::start(&["--no-open"]);
    let post = |headers: &str, length: usize| {
        let head = format!("POST /api/read HTTP/1.1\r\n{headers}\r\nContent-Length: {length}");
        server.send(&head, &qr).0
    };
    let local = "Host: 127.0.0.1:{port}";
    assert_eq!(post(local, qr.len()), 200);
    for headers in [
        "Host: evil.example\r\nHost: 127.0.0.1:{port}",
        "Host: 127.0.0.1:{port}\r\nHost: 127.0.0.1:{port}",
        "Host: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}",
        "Host: 127.0.0.1:{port}\r\nContent-Length: 1",
        "Host: 127.0.0.1:{port}\r\nTransfer-Encoding: chunked",
        "Host: 127.0.0.1:{port}\r\nX-File-Name: a.png\r\nX-File-Name: b.png",
        "Host: 127.0.0.1:{port}\r\nOrigin : http://evil.example",
        "Host: 127.0.0.1:{port}\r\nBad Header: value",
        "Host: 127.0.0.1:{port}\r\nNoColon",
    ] {
        assert_eq!(post(headers, qr.len()), 400, "{headers}");
    }
    // A body shorter than its length.
    assert_eq!(post(local, qr.len() + 10), 400);
    let length = format!("Content-Length: {}", qr.len());
    let head = format!("POST /api/read HTTP/1.1\r\n{local}\r\n{length}");
    for head in [
        head.replace("Length: ", "Length: +"),
        head.replace(" HTTP/1.1", ""),
        head.replace("HTTP/1.1", "XYZ"),
    ] {
        assert_eq!(server.send(&head, &qr).0, 400, "{head}");
    }
    // Non-UTF-8 bytes: fine in an unknown header's value, never in the request line.
    let raw = |line: &[u8], header: &[u8]| {
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let host = format!("\r\nHost: 127.0.0.1:{}\r\n", server.port);
        stream
            .write_all(&[line, host.as_bytes(), header, b"\r\n\r\n"].concat())
            .unwrap();
        let mut response = Vec::new();
        // A server that drops the request may reset the connection; that reads as no status.
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(response.get(9..12).unwrap_or_default()).into_owned()
    };
    assert_eq!(raw(b"GET / HTTP/1.1", b"X-Other: \xe9\xff"), "200");
    assert_eq!(raw(b"GET /\xe9 HTTP/1.1", b"X-Other: a"), "400");
    assert_eq!(raw(b"GET / HTTP/1.1", b"\xe9: x"), "400");
}

#[test]
fn reads_one_file_at_a_time_while_a_client_stalls() {
    let server = Server::start(&["--no-open"]);
    let _stalled = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    // Each upload has its own name and payload, so a mixed-up answer shows.
    let uploads: Vec<_> = (0..8)
        .map(|i| {
            let qr = code(BarcodeFormat::QRCode, &format!("upload {i}"), "");
            (format!("{i}.png"), encode(qr, ImageFormat::Png))
        })
        .collect();
    let answers: Vec<(u16, Value)> = std::thread::scope(|s| {
        let reads: Vec<_> = uploads
            .iter()
            .map(|(name, png)| s.spawn(|| server.read(name, "{}", png)))
            .collect();
        reads.into_iter().map(|r| r.join().unwrap()).collect()
    });
    for (i, (status, file)) in answers.iter().enumerate() {
        assert_eq!(*status, 200);
        assert_eq!(file["path"], format!("{i}.png"));
        assert_eq!(file["barcodes"][0]["text"], format!("upload {i}"));
    }
}

#[test]
fn refuses_a_nul_or_bare_cr_in_a_header_value() {
    let server = Server::start(&["--no-open"]);
    let local = "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{port}";
    // RFC 9110 5.5 and RFC 9112 2.2: such a value is invalid, whichever header holds it.
    for header in [
        "X-File-Name: qr\0hidden.png",
        "X-File-Name: qr\rhidden.png",
        "X-Other: a\0b",
    ] {
        let head = format!("{local}\r\n{header}\r\nContent-Length: 0");
        assert_eq!(server.send(&head, b"").0, 400, "{header:?}");
    }
}

#[test]
fn refuses_a_bare_cr_at_the_end_of_a_header_value() {
    let server = Server::start(&["--no-open"]);
    let local = "POST /api/read HTTP/1.1\r\nHost: 127.0.0.1:{port}";
    for header in ["X-File-Name: qr.png\r", "X-Other: a\r ", "X-Other: a \r\t"] {
        let head = format!("{local}\r\n{header}\r\nContent-Length: 0");
        assert_eq!(server.send(&head, b"").0, 400, "{header:?}");
    }
}

#[test]
fn reports_a_busy_port_and_shows_its_options() {
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let out = Command::new(BIN)
        .args(["ui", "--no-open", "--port", &port])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!(
            "✗ Can't start the page: port {port} is in use.\n  Leave out --port to pick a free one.\n"
        )
    );
    let bad = Command::new(BIN)
        .args(["ui", "--port", "x"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let help = Command::new(BIN).args(["ui", "--help"]).output().unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    for part in [
        "Open the local web page.\n",
        "--port <N>  port to listen on [default: a free one]\n",
        "--no-open   don't open the browser\n",
        "  barcode-read ui\n",
    ] {
        assert!(help.contains(part), "{help}");
    }
}

#[test]
fn ends_quietly_when_stdout_closes() {
    // The read end closes before the spawn: dropped after it, a test spawning beside this one
    // could inherit a copy, and the server's first write would succeed.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = Command::new(BIN)
        .args(["ui", "--no-open"])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).as_ref()
        ),
        (Some(0), "")
    );
}

/// A stand-in for xdg-open and open that writes down the address it was given.
#[cfg(unix)]
#[test]
fn opens_the_browser_unless_told_not_to() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ui-browser");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let opened = dir.join("opened");
    for opener in ["xdg-open", "open"] {
        let script = dir.join(opener);
        fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1\" > '{}'\n", opened.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
    let start = |args: &[&str]| {
        let mut child = Command::new(BIN)
            .arg("ui")
            .args(args)
            .env("PATH", &path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        child.kill().unwrap();
        child.wait().unwrap();
        line
    };
    start(&["--no-open"]);
    assert!(!opened.exists());
    let line = start(&[]);
    let url = line.split_whitespace().nth(1).unwrap();
    assert_eq!(fs::read_to_string(&opened).unwrap(), format!("{url}\n"));
}

#[cfg(windows)]
#[test]
fn the_exe_carries_the_icon() {
    let ico = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icon.ico")).unwrap();
    let exe = fs::read(BIN).unwrap();
    // The 256 px entry is a PNG, stored in the exe's resources byte for byte.
    let png = &ico[ico.windows(4).position(|w| w == b"\x89PNG").unwrap()..];
    assert!(exe.windows(png.len()).any(|w| w == png));
}

/// A console of its own, as Explorer gives a double-clicked exe: the page starts. Run by CI
/// (`--ignored`), since it opens the default browser.
#[cfg(windows)]
#[test]
#[ignore = "opens the default browser"]
fn a_double_click_starts_the_page() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let mut child = Command::new(BIN)
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .strip_prefix("Open http://127.0.0.1:")
        .and_then(|rest| rest.strip_suffix(" · close this window to stop.\n"))
        .unwrap_or_else(|| panic!("{line:?}"));
    let server = Server {
        child,
        port: port.parse().unwrap(),
    };
    let (status, _, body) = server.send("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}", b"");
    assert_eq!((status, body.as_str()), (200, PAGE));
}
