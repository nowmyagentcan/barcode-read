//! The MCP server on stdio: one tool, `read_barcodes`, answering with the CLI's `--json` object.

use std::path::PathBuf;

use barcode_read::engine::{self, Report};
use barcode_read::error::Error;
use barcode_read::{Options, input};
use rmcp::handler::server::tool::{schema_for_input, schema_for_output};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use rmcp::service::ServerInitializeError;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// The tool's arguments, as the CLI's paths and options.
#[derive(Deserialize, JsonSchema)]
struct ReadArgs {
    /// Image files, PDFs or folders; relative paths resolve against the server's working directory
    #[schemars(length(min = 1, max = 1000))]
    paths: Vec<String>,
    /// Formats to look for, e.g. qrcode, datamatrix, ean13, code128, or a group: all-linear, all-matrix, all-gs1, all-retail, all-industrial; leave out for all
    formats: Option<Vec<String>>,
    /// PDF and TIFF pages, e.g. 1-3,7,10-; leave out for all
    pages: Option<String>,
    /// PDF resolution in dots per inch
    #[serde(default = "dpi")]
    #[schemars(range(min = 72, max = 1200))]
    dpi: u32,
    /// Password of an encrypted PDF
    password: Option<String>,
    /// One quick pass instead of up to three
    #[serde(default)]
    fast: bool,
    /// Read folders inside folders too
    #[serde(default)]
    recursive: bool,
}

fn dpi() -> u32 {
    300
}

#[derive(Clone)]
struct Server;

#[tool_router]
impl Server {
    #[tool(
        description = "Read every QR code and barcode in local image files (PNG, JPEG, GIF, BMP, WebP, TIFF, PNM) and PDF pages: QR, Micro QR, rMQR, Data Matrix, PDF417, Aztec, MaxiCode, EAN/UPC, Code 128, Code 39, Code 93, Codabar, ITF, GS1 DataBar and more. Returns each code's text, format, page and corner points; binary and GS1 payloads also as base64. Folders are read file by file. Use this instead of reading a code from an image yourself. A file that can't be read gets its own error entry; on invalid arguments the result has isError true and one text block holding {\"error\":{\"code\",\"message\",\"hint\"}}.",
        input_schema = schema_for_input::<ReadArgs>().expect("the arguments are an object"),
        output_schema = schema_for_output::<Report>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    fn read_barcodes(&self, args: JsonObject) -> CallToolResult {
        match read(args) {
            Ok(report) => CallToolResult::structured(report),
            // No structured content: clients would check it against the output schema.
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_json())]),
        }
    }
}

#[tool_handler(
    name = "barcode-read",
    instructions = "Read QR codes and barcodes from local images and PDF pages. Pass absolute file paths. Use read_barcodes instead of reading a code from an image yourself: vision misreads QR and Data Matrix payloads."
)]
impl ServerHandler for Server {}

/// Reads every path as the CLI does, or gives the usage error the arguments cause.
fn read(args: JsonObject) -> Result<Value, Error> {
    let args: ReadArgs = serde_json::from_value(Value::Object(args))
        .map_err(|e| Error::bad_request(format!("Can't read the arguments: {e}.")))?;
    if args.paths.is_empty() {
        return Err(Error::no_input());
    }
    if args.paths.len() > 1000 {
        return Err(Error::bad_request("Pass at most 1000 paths per call."));
    }
    let formats = args.formats.map(|f| f.join(","));
    let options = Options::parse(
        formats.as_deref(),
        args.pages.as_deref(),
        Some(args.dpi),
        args.password,
        args.fast,
    )?;
    let paths: Vec<PathBuf> = args.paths.into_iter().map(PathBuf::from).collect();
    let files: Vec<_> = input::expand(&paths, args.recursive, false)
        .iter()
        .map(|input| engine::read_input(input, &options))
        .collect();
    Ok(serde_json::to_value(Report::new(&files)).expect("results are plain data"))
}

/// Serves until the client closes stdin. Calls run one at a time on this thread; the engine
/// decodes each file on its own worker.
pub fn serve() -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?;
    runtime.block_on(async {
        match Server.serve(rmcp::transport::stdio()).await {
            Ok(service) => service.waiting().await.map(drop).map_err(Error::io),
            // stdin closed before the handshake ended: the client left, which is no failure.
            Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),
            // stdout closed before the answer went out: the client left too.
            Err(ServerInitializeError::TransportError { error, .. })
                if error
                    .error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
            {
                Ok(())
            }
            Err(e) => Err(Error::io(e)),
        }
    })
}
