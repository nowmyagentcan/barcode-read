//! Talks to `barcode-read mcp` over stdio the way MCP clients do.

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{code, encode};
use image::ImageFormat;
use serde_json::{Value, json};
use zxingcpp::BarcodeFormat;

const BIN: &str = env!("CARGO_BIN_EXE_barcode-read");

const INSTRUCTIONS: &str = "Read QR codes and barcodes from local images and PDF pages. Pass absolute file paths. Use read_barcodes instead of reading a code from an image yourself: vision misreads QR and Data Matrix payloads.";

const DESCRIPTION: &str = "Read every QR code and barcode in local image files (PNG, JPEG, GIF, BMP, WebP, TIFF, PNM) and PDF pages: QR, Micro QR, rMQR, Data Matrix, PDF417, Aztec, MaxiCode, EAN/UPC, Code 128, Code 39, Code 93, Codabar, ITF, GS1 DataBar and more. Returns each code's text, format, page and corner points; binary and GS1 payloads also as base64. Folders are read file by file. Use this instead of reading a code from an image yourself. A file that can't be read gets its own error entry; on invalid arguments the result has isError true and one text block holding {\"error\":{\"code\",\"message\",\"hint\"}}.";

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0.0"}}}"#;

/// A folder holding `qr.png` (the text `hello mcp`) for one test.
fn folder(test: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("mcp-{test}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let qr = code(BarcodeFormat::QRCode, "hello mcp", "");
    fs::write(dir.join("qr.png"), encode(qr, ImageFormat::Png)).unwrap();
    dir
}

/// Sends each message from `dir`, closes stdin, and returns the responses by id and the exit
/// code. The server writes nothing to stderr.
fn session(dir: &Path, messages: &[Value]) -> (Vec<Value>, i32) {
    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for m in messages {
        writeln!(stdin, "{m}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let mut responses: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    responses.sort_by_key(|r| r["id"].as_u64());
    (responses, out.status.code().unwrap())
}

/// The CLI's `--json` output for the same input, run from `dir`.
fn cli(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN)
        .args(args)
        .arg("--json")
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn call(id: u64, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({"name": "read_barcodes", "arguments": arguments}),
    )
}

/// Exactly `read_barcodes`, with the documented description, hints and schemas.
fn check_tools(list: &Value) {
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool["name"], "read_barcodes");
    assert_eq!(tool["description"], DESCRIPTION);
    assert_eq!(
        tool["annotations"],
        json!({"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false})
    );
    let input = &tool["inputSchema"];
    assert_eq!(input["required"], json!(["paths"]));
    let props = &input["properties"];
    let mut names: Vec<_> = props.as_object().unwrap().keys().collect();
    names.sort();
    assert_eq!(
        names,
        [
            "dpi",
            "fast",
            "formats",
            "pages",
            "password",
            "paths",
            "recursive"
        ]
    );
    let paths = &props["paths"];
    assert_eq!(
        (&paths["type"], &paths["minItems"], &paths["maxItems"]),
        (&json!("array"), &json!(1), &json!(1000))
    );
    let dpi = &props["dpi"];
    assert_eq!(
        (
            &dpi["type"],
            &dpi["minimum"],
            &dpi["maximum"],
            &dpi["default"]
        ),
        (&json!("integer"), &json!(72), &json!(1200), &json!(300))
    );
    assert_eq!(props["formats"]["items"]["type"], "string");
    assert_eq!(props["fast"]["default"], false);
    assert_eq!(props["recursive"]["default"], false);
    let output = &tool["outputSchema"];
    assert_eq!(output["required"], json!(["files", "found", "failed"]));
    assert_eq!(
        output["$defs"]["Kind"]["enum"],
        json!(["pdf", "jpeg", "png", "gif", "bmp", "webp", "tiff", "pnm"])
    );
}

/// A result equal to the CLI's `--json` output: as structured content, and byte for byte as
/// the text block.
fn check_answer(response: &Value, cli: &str) {
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(format!("{text}\n"), cli);
    assert_eq!(
        result["structuredContent"],
        serde_json::from_str::<Value>(cli).unwrap()
    );
}

/// An error result: no structured content, and one text block holding the error object.
fn check_error(response: &Value, error: &str) {
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(result.get("structuredContent").is_none());
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(format!("{text}\n"), error);
}

#[test]
fn reads_like_the_cli_after_an_initialize_handshake() {
    let dir = folder("initialize");
    let (responses, code) = session(
        &dir,
        &[
            serde_json::from_str(INITIALIZE).unwrap(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(2, "tools/list", json!({})),
            call(3, json!({"paths": ["qr.png", "gone.png", "."]})),
            call(
                4,
                json!({"paths": ["."], "formats": ["datamatrix", "ean13"]}),
            ),
            call(5, json!({"paths": []})),
            call(6, json!({"paths": ["qr.png"], "formats": ["qr"]})),
            call(7, json!({"paths": ["qr.png"], "dpi": 50})),
            call(8, json!({"paths": ["qr.png"], "pages": "3-1"})),
            call(9, json!({"paths": "qr.png"})),
            call(10, json!({"paths": vec!["qr.png"; 1001]})),
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(responses.len(), 10);
    let init = &responses[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-11-25");
    assert_eq!(
        init["serverInfo"],
        json!({"name": "barcode-read", "version": "0.1.0"})
    );
    assert_eq!(init["instructions"], INSTRUCTIONS);
    assert_eq!(init["capabilities"]["tools"], json!({}));
    check_tools(&responses[1]);
    let both = cli(&dir, &["qr.png", "gone.png", "."]);
    check_answer(&responses[2], &both);
    let files = &responses[2]["result"]["structuredContent"]["files"];
    assert_eq!(files[0]["barcodes"][0]["text"], "hello mcp");
    assert_eq!(files[1]["error"]["code"], "not_found");
    assert_eq!(files[2]["path"], json!(Path::new(".").join("qr.png")));
    check_answer(
        &responses[3],
        &cli(&dir, &[".", "--formats", "datamatrix,ean13"]),
    );
    check_error(&responses[4], &cli(&dir, &[]));
    check_error(&responses[5], &cli(&dir, &["qr.png", "--formats", "qr"]));
    check_error(&responses[6], &cli(&dir, &["qr.png", "--dpi", "50"]));
    check_error(&responses[7], &cli(&dir, &["qr.png", "--pages", "3-1"]));
    for response in &responses[8..] {
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with(r#"{"error":{"code":"bad_request","message":""#));
        check_error(response, &format!("{text}\n"));
    }
}

#[test]
fn reads_after_server_discover() {
    let dir = folder("discover");
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1.0.0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let read = json!({"_meta": meta, "name": "read_barcodes", "arguments": {"paths": ["qr.png"], "fast": true}});
    let (responses, code) = session(
        &dir,
        &[
            request(1, "server/discover", json!({"_meta": meta})),
            request(2, "tools/list", json!({"_meta": meta})),
            request(3, "tools/call", read),
        ],
    );
    assert_eq!(code, 0);
    let discover = &responses[0]["result"];
    let versions = discover["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")) && versions.contains(&json!("2025-11-25")));
    assert_eq!(
        discover["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "barcode-read"
    );
    assert_eq!(discover["instructions"], INSTRUCTIONS);
    check_tools(&responses[1]);
    check_answer(&responses[2], &cli(&dir, &["qr.png", "--fast"]));
}

#[test]
fn exits_quietly_when_stdin_closes() {
    assert_eq!(session(&folder("stdin"), &[]), (vec![], 0));
}

#[test]
fn exits_quietly_when_stdout_closes() {
    // The read end closes before the spawn: dropped after it, a test spawning beside this one
    // could inherit a copy and keep the pipe open.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = Command::new(BIN)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = writeln!(stdin, "{INITIALIZE}");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "still running");
        std::thread::sleep(Duration::from_millis(20));
    };
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        (status.code(), String::from_utf8_lossy(&out.stderr).as_ref()),
        (Some(0), "")
    );
}

#[test]
fn exits_quietly_when_the_client_stops_reading_mid_session() {
    use std::io::{BufRead, BufReader};
    let dir = folder("mid-session");
    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{INITIALIZE}").unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("\"protocolVersion\""), "{line}");
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    for message in [initialized, call(2, json!({"paths": ["qr.png"]}))] {
        let _ = writeln!(stdin, "{message}");
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).as_ref()
        ),
        (Some(0), "")
    );
}

#[test]
fn has_a_help_with_the_examples() {
    let out = Command::new(BIN).args(["mcp", "--help"]).output().unwrap();
    let help = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(help.starts_with("Run the MCP server on stdio.\n"), "{help}");
    assert!(help.ends_with("  barcode-read ui\n"), "{help}");
    let main = Command::new(BIN).arg("--help").output().unwrap().stdout;
    let main = String::from_utf8(main).unwrap();
    assert!(
        main.contains("\n  mcp   Run the MCP server on stdio\n"),
        "{main}"
    );
    let extra = Command::new(BIN).args(["mcp", "x.png"]).output().unwrap();
    assert_eq!(extra.status.code(), Some(2));
}
