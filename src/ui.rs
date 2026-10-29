//! The local page on 127.0.0.1: `GET /` serves it and `POST /api/read` reads one uploaded file.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use barcode_read::Options;
use barcode_read::engine::{self, Failure};
use barcode_read::error::Error;
use serde::Deserialize;

const PAGE: &str = include_str!("ui.html");
/// The most a request may send in its line and headers.
const HEAD_LIMIT: u64 = 16 * 1024;
/// The largest upload.
const UPLOAD_LIMIT: u64 = 256 << 20;
/// The page reaches nothing but its own server.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; connect-src 'self'; base-uri 'none'; form-action 'none'";
const JSON: &str = "application/json";

/// Requests are answered side by side, but files are read one at a time: each can take a few
/// hundred megabytes.
static DECODE: Mutex<()> = Mutex::new(());

type Response = (u16, &'static str, String);

/// The options the page sends in `X-Options`, as the CLI's flags.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PageOptions {
    formats: Option<String>,
    pages: Option<String>,
    dpi: Option<u32>,
    password: Option<String>,
    #[serde(default)]
    fast: bool,
}

/// Serves on 127.0.0.1:`port` (0 picks a free port) until the process is stopped, and opens
/// the page in the default browser when `open` is set. `stop` says how to stop it.
pub fn serve(port: u16, open: bool, stop: &str) -> Result<(), Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|e| {
        if e.kind() == io::ErrorKind::AddrInUse {
            Error::port_in_use(port)
        } else {
            Error::new("io", format!("Can't start the page: {e}."), None)
        }
    })?;
    let port = listener.local_addr().map_or(port, |a| a.port());
    let url = format!("http://127.0.0.1:{port}");
    // With stdout closed nobody learns the address, so end quietly as other commands do.
    match writeln!(io::stdout(), "Open {url}{stop}") {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
        Err(e) => return Err(Error::io(e)),
        Ok(()) => {}
    }
    if open {
        browse(url);
    }
    for stream in listener.incoming().flatten() {
        // A client that hangs up or stalls loses only its own connection.
        thread::spawn(move || handle(&stream, port));
    }
    Ok(())
}

/// Asks the OS to open `url`; when it can't, the printed address is enough.
fn browse(url: String) {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Some openers wait for the browser to close, so they run beside the server.
    thread::spawn(move || command.status());
}

/// One request, one response, then the connection closes.
fn handle(stream: &TcpStream, port: u16) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let (status, kind, body) = respond(&mut BufReader::new(stream.take(HEAD_LIMIT)), port)?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Content Too Large",
        _ => "Unprocessable Content",
    };
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: {CSP}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.shutdown(Shutdown::Write)?;
    // Read what the client still sends, so closing doesn't reset the connection before it
    // has read the answer.
    io::copy(&mut stream.take(1 << 20), &mut io::sink()).map(drop)
}

fn respond(reader: &mut BufReader<io::Take<&TcpStream>>, port: u16) -> io::Result<Response> {
    // Lines are bytes: a header value may carry opaque ones (RFC 9110 §5.5).
    let (mut request, mut line) = (Vec::new(), Vec::new());
    reader.read_until(b'\n', &mut request)?;
    let (mut host, mut origin, mut length, mut name, mut options) = (None, None, None, None, None);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(match reader.get_ref().limit() {
                0 => error(
                    413,
                    "too_large",
                    "The request headers are larger than 16 KB.",
                ),
                _ => error(400, "bad_request", "The request ended early."),
            });
        }
        if line.iter().all(|b| b"\r\n".contains(b)) {
            break;
        }
        let token = |n: &[u8]| {
            !n.is_empty()
                && n.iter()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(b))
        };
        let Some(colon) = line
            .iter()
            .position(|&b| b == b':')
            .filter(|&i| token(&line[..i]))
        else {
            return Ok(error(400, "bad_request", "Can't read a request header."));
        };
        let field = match line[..colon].to_ascii_lowercase().as_slice() {
            b"host" => &mut host,
            b"origin" => &mut origin,
            b"content-length" => &mut length,
            b"x-file-name" => &mut name,
            b"x-options" => &mut options,
            // Bodies come only with a Content-Length; chunked ones aren't read.
            b"transfer-encoding" => {
                return Ok(error(
                    400,
                    "bad_request",
                    "Send the body with a Content-Length.",
                ));
            }
            _ => continue,
        };
        let value = String::from_utf8_lossy(&line[colon + 1..]);
        // A second value could disagree with the first one checked (RFC 9112 §3.2, §6.3).
        if field.replace(value.trim().to_owned()).is_some() {
            return Ok(error(400, "bad_request", "A request header is repeated."));
        }
    }
    // RFC 9112 §3: the request line is ASCII.
    let request = std::str::from_utf8(&request).ok().filter(|r| r.is_ascii());
    let mut parts = request
        .unwrap_or_default()
        .trim_end_matches(['\r', '\n'])
        .split(' ');
    let (Some(method), Some(target), Some("HTTP/1.1" | "HTTP/1.0"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Ok(error(400, "bad_request", "Can't read the request line."));
    };
    let local = |scheme: &str, value: &str| {
        ["127.0.0.1", "localhost"]
            .iter()
            .any(|h| value.eq_ignore_ascii_case(&format!("{scheme}{h}:{port}")))
    };
    if !host.is_some_and(|h| local("", &h)) || origin.is_some_and(|o| !local("http://", &o)) {
        return Ok(error(
            403,
            "forbidden",
            "Only this computer's page can use barcode-read ui.",
        ));
    }
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    match (method, path) {
        ("GET", "/") => Ok((200, "text/html; charset=utf-8", PAGE.into())),
        ("POST", "/api/read") => {
            // Digits only: Rust would also read a leading `+`.
            let digits = |l: &String| l.bytes().all(|b| b.is_ascii_digit());
            let Some(length) = length.map_or(Some(0), |l| l.parse().ok().filter(|_| digits(&l)))
            else {
                return Ok(error(400, "bad_request", "Can't read the request length."));
            };
            if length > UPLOAD_LIMIT {
                return Ok(error(413, "too_large", "The file is larger than 256 MiB."));
            }
            let buffered = reader.buffer().len() as u64;
            reader.get_mut().set_limit(length.saturating_sub(buffered));
            let mut body = Vec::new();
            reader.take(length).read_to_end(&mut body)?;
            if (body.len() as u64) < length {
                return Ok(error(400, "bad_request", "The request ended early."));
            }
            Ok(match read(name.as_deref(), options.as_deref(), body) {
                Ok(file) => (200, JSON, file),
                Err(e) if e.code == "bad_request" => (400, JSON, failure(&e)),
                Err(e) => (422, JSON, failure(&e)),
            })
        }
        _ => Ok(error(404, "not_found", "There's nothing here.")),
    }
}

/// Reads one uploaded file as the JSON `files[0]` object of the CLI.
fn read(name: Option<&str>, options: Option<&str>, bytes: Vec<u8>) -> Result<String, Error> {
    let name = unescape(name.unwrap_or_default())
        .ok_or_else(|| Error::bad_request("Can't read the file name."))?;
    let o: PageOptions = options
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| Error::bad_request(format!("Can't read the options: {e}.")))?
        .unwrap_or_default();
    let options = Options::parse(
        o.formats.as_deref(),
        o.pages.as_deref(),
        o.dpi,
        o.password,
        o.fast,
    )?;
    let _one_at_a_time = DECODE.lock().unwrap_or_else(|e| e.into_inner());
    let file = engine::read(name, bytes, &options);
    Ok(serde_json::to_string(&file).expect("results are plain data"))
}

/// Percent-decodes `s` as UTF-8.
fn unescape(s: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        rest = tail;
        if b == b'%' {
            let hex = tail
                .get(..2)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))?;
            bytes.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            rest = &tail[2..];
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).ok()
}

fn failure(e: &Error) -> String {
    serde_json::to_string(&Failure { error: e }).expect("plain strings")
}

fn error(status: u16, code: &'static str, message: &str) -> Response {
    (status, JSON, failure(&Error::new(code, message, None)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_percent_decoded_utf_8() {
        assert_eq!(
            unescape("Gr%C3%BC%C3%9Fe%20scan.pdf").as_deref(),
            Some("Grüße scan.pdf")
        );
        assert_eq!(unescape("plain.png").as_deref(), Some("plain.png"));
        for bad in ["%", "%4", "%zz", "%+1", "%C3"] {
            assert_eq!(unescape(bad), None, "{bad}");
        }
    }
}
