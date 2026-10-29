# barcode-read

Read QR codes and barcodes from photos, scans and PDFs: a command line tool with JSON output, a local web page, and an MCP server for AI agents. Everything runs on your computer.

## Install

Windows (PowerShell, no admin; installs to `%LOCALAPPDATA%\Programs\barcode-read` and adds it to your user PATH):

```powershell
irm https://github.com/nowmyagentcan/barcode-read/releases/latest/download/install.ps1 | iex
```

macOS and Linux (installs to `~/.local/bin`; Linux needs glibc 2.35 or newer, so Ubuntu 22.04, Debian 12, Fedora 36 or later):

```sh
curl -fsSL https://github.com/nowmyagentcan/barcode-read/releases/latest/download/install.sh | sh
```

Both scripts check the archive's SHA-256 against `SHA256SUMS` before installing anything. Or download an archive yourself; each holds one `barcode-read` binary that needs no runtime:

| System | Download |
|---|---|
| Windows x64 | [barcode-read-v0.1.0-windows-x64.zip](https://github.com/nowmyagentcan/barcode-read/releases/download/v0.1.0/barcode-read-v0.1.0-windows-x64.zip) |
| macOS Apple silicon | [barcode-read-v0.1.0-macos-arm64.tar.gz](https://github.com/nowmyagentcan/barcode-read/releases/download/v0.1.0/barcode-read-v0.1.0-macos-arm64.tar.gz) |
| macOS Intel | [barcode-read-v0.1.0-macos-x64.tar.gz](https://github.com/nowmyagentcan/barcode-read/releases/download/v0.1.0/barcode-read-v0.1.0-macos-x64.tar.gz) |
| Linux x64 | [barcode-read-v0.1.0-linux-x64.tar.gz](https://github.com/nowmyagentcan/barcode-read/releases/download/v0.1.0/barcode-read-v0.1.0-linux-x64.tar.gz) |
| Linux arm64 | [barcode-read-v0.1.0-linux-arm64.tar.gz](https://github.com/nowmyagentcan/barcode-read/releases/download/v0.1.0/barcode-read-v0.1.0-linux-arm64.tar.gz) |

With Rust, CMake and a C++20 compiler: `cargo install --git https://github.com/nowmyagentcan/barcode-read --tag v0.1.0 --locked`.
To remove it, delete the binary (on Windows, the `barcode-read` folder above).

## Use

```sh
barcode-read photo.jpg                              # every code in a picture
barcode-read scan.pdf --pages 1-3                   # PDF pages, rendered at 300 dpi
barcode-read labels/ --formats datamatrix,code128   # a folder, only these formats
cat code.png | barcode-read -                       # stdin
barcode-read                                        # interactive: drop files, /help, /quit
```

```text
$ barcode-read label.png fax.tif
label.png  QR Code
  barcode-read smoke test
fax.tif  page 2  QR Code
  fax page 2
2 codes in 2 of 2 files
```

- Binary and GS1 payloads are marked as such; control and bidi characters in a payload print as `<ESC>`, `<U+202E>` and so on, so a code can't change your terminal.
- Exit codes: 0 every file read and a code found, 1 no code found or a file failed (the others are still reported), 2 usage, 3 runtime.

**The page:** `barcode-read ui` opens a small page in your browser, served on `127.0.0.1` only. Drop images or PDFs, choose them, or paste a screenshot with Ctrl+V. Copy all gives the text, Save a `barcodes.csv`. On Windows, double-clicking `barcode-read.exe` opens the same page.

### Use with AI agents

Agents can run the CLI with `--json` (one JSON object on stdout; errors as `{"error":{"code","message","hint"}}`):

```sh
barcode-read photo.jpg --json
barcode-read scans/ --formats qrcode,datamatrix --json
barcode-read invoice.pdf --pages 1 --json
```

```json
{"files":[{"path":"label.png","type":"png","pages":[{"page":1,"width":132,"height":132}],"barcodes":[{"page":1,"format":"QR Code","text":"barcode-read smoke test","content":"text","bytes_base64":null,"symbology_id":"]Q1","position":[[16,16],[116,16],[116,116],[16,116]],"rotation":0,"ec_level":"M","version":"2","sequence":null}],"warnings":[],"error":null}],"found":1,"failed":0}
```

Or add the local MCP server (stdio, one tool `read_barcodes` taking file and folder paths):

| Agent | Setup |
|---|---|
| Claude Code | `claude mcp add barcode-read -- barcode-read mcp` |
| Codex | `codex mcp add barcode-read -- barcode-read mcp` |
| Cursor (`~/.cursor/mcp.json`), Claude Desktop | `{"mcpServers":{"barcode-read":{"command":"barcode-read","args":["mcp"]}}}` |
| VS Code (`.vscode/mcp.json`) | `{"servers":{"barcode-read":{"type":"stdio","command":"barcode-read","args":["mcp"]}}}` |

The tool returns the CLI's `--json` object as `structuredContent` and as text. A file that can't be read gets its own error entry; invalid arguments return `isError: true` with one text block holding the error object. Agents that save a pasted image to a file can pass its path; vision models often misread QR and Data Matrix payloads, which this avoids.

## Supported inputs and limits

- **Formats:** QR Code, Micro QR, rMQR, Data Matrix, PDF417, MicroPDF417, Aztec, MaxiCode, EAN-13, EAN-8, UPC-A, UPC-E, Code 128, Code 39, Code 93, Codabar, ITF, GS1 DataBar (Omni, Limited, Expanded), DX Film Edge and more; `barcode-read --help` lists every name and the groups `all-linear`, `all-matrix`, `all-gs1`, `all-retail`, `all-industrial`.
- **UPC:** UPC-A and UPC-E come back as their 13-digit GTIN (UPC-A shows as EAN-13 unless you pass `--formats upca`), as zxing-cpp reports them.
- **Files:** PNG, JPEG, GIF and WebP (first frame), BMP, TIFF (every page, fax included), PNM and PDF. The type comes from the file's bytes, not its name. Folders are read by extension, in name order, hidden files skipped; `-r` goes into subfolders.
- **PDF:** each page is rendered at `--dpi` (default 300); a page over 40 megapixels is rendered at a lower dpi with a warning. Encrypted PDFs take `--password`.
- **Limits:** 1 GiB per file; images up to 120 megapixels and 65,535 pixels per side.
- **Hard pictures:** when the first pass finds nothing, a second pass binarizes the whole image differently and, for images up to 1000 pixels, a third reads it at twice the size. `--fast` runs one pass.
- **Not read:** HEIC, AVIF, JPEG XL, JPEG 2000 and SVG (each gets its own error and a hint), camera or screen capture, URLs. Structured-append parts are reported with their sequence number, not merged. It doesn't write barcodes.
- It makes no network requests; `ui` only listens on `127.0.0.1`.

## How it works

Each file is turned into 8-bit grey pixels (JPEG by [zune-jpeg](https://github.com/etemesi254/zune-image), other images by [image](https://github.com/image-rs/image) and [tiff](https://github.com/image-rs/image-tiff), PDF pages by [hayro](https://github.com/LaurenzV/hayro)), one page at a time.
[zxing-cpp](https://github.com/zxing-cpp/zxing-cpp) finds and decodes the codes, in up to three passes; corner points are mapped to the picture as displayed, EXIF rotation included.
The CLI uses [clap](https://github.com/clap-rs/clap) and the MCP server the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).

## First run on an unsigned build

Windows: if "Windows protected your PC" appears, choose More info, then Run anyway.
The install script doesn't trigger it; a zip extracted in Explorer may.

macOS: if it says the developer can't be verified, open System Settings, Privacy & Security, and choose Open Anyway.
The install script doesn't trigger it, because curl downloads carry no quarantine flag.

## Licence

Apache-2.0, see [LICENSE](LICENSE). Each release archive includes `THIRD-PARTY-LICENSES.txt` for the bundled dependencies, including zxing-cpp's zint and libzueci parts.

---

[nowmyagentcan.com](https://nowmyagentcan.com) · [x.com/chayp05](https://x.com/chayp05)
