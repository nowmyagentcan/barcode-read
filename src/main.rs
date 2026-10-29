//! The command line: `barcode-read [read] <PATH>... [OPTIONS]`.

mod interactive;
mod mcp;
mod text;
mod ui;

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use barcode_read::Options;
use barcode_read::engine::{self, Report};
use barcode_read::error::Error;
use barcode_read::input;
use clap::{ArgAction, CommandFactory, FromArgMatches, Parser};
use text::{Look, Out};
use zxingcpp::{BarcodeFormat, BarcodeFormats};

const HELP: &str = "\
{about}

{usage-heading} {usage}

Commands:
  read  Read codes from files and folders (the default command)
  ui    Open the local web page
  mcp   Run the MCP server on stdio

Options:
{options}{after-help}";

const EXAMPLES: &str = "\
Examples:
  barcode-read photo.jpg
  barcode-read scan.pdf --pages 1-3 --json
  barcode-read labels/ --formats datamatrix,code128
  barcode-read invoice.pdf --dpi 600
  cat code.png | barcode-read -
  barcode-read ui";

/// `barcode-read ui`
#[derive(Parser)]
#[command(
    name = "barcode-read ui",
    about = "Open the local web page.",
    after_help = EXAMPLES,
    disable_version_flag = true
)]
struct Ui {
    /// port to listen on [default: a free one]
    #[arg(long, value_name = "N", default_value_t = 0, hide_default_value = true)]
    port: u16,
    /// don't open the browser
    #[arg(long)]
    no_open: bool,
}

/// `barcode-read mcp`
#[derive(Parser)]
#[command(
    name = "barcode-read mcp",
    about = "Run the MCP server on stdio.",
    after_help = EXAMPLES,
    disable_version_flag = true
)]
struct Mcp {}

#[derive(Parser, Default)]
#[command(
    name = "barcode-read",
    version,
    about = "Read QR codes and barcodes from photos, scans and PDFs.",
    help_template = HELP,
    override_usage = "barcode-read [COMMAND] [PATH]...",
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Cli {
    #[arg(value_name = "PATH", hide = true)]
    paths: Vec<PathBuf>,
    /// formats to look for, e.g. qrcode,datamatrix,ean13 [default: all]
    #[arg(long, value_name = "LIST")]
    formats: Option<String>,
    /// PDF and TIFF pages, e.g. 1-3,7,10- [default: all]
    #[arg(long, value_name = "LIST")]
    pages: Option<String>,
    /// PDF resolution, 72-1200 [default: 300]
    #[arg(long, value_name = "N")]
    dpi: Option<String>,
    /// password of an encrypted PDF
    #[arg(long, value_name = "PW")]
    password: Option<String>,
    /// one quick pass instead of up to three
    #[arg(long)]
    fast: bool,
    /// read folders inside folders too
    #[arg(short, long)]
    recursive: bool,
    /// print one JSON object
    #[arg(long)]
    json: bool,
    /// show causes and per-pass timings
    #[arg(long)]
    debug: bool,
    /// print help
    #[arg(short, long, action = ArgAction::Help)]
    help: Option<bool>,
    /// print version
    #[arg(short = 'V', long, action = ArgAction::Version)]
    version: Option<bool>,
}

impl Cli {
    /// The reading options, or the usage error a value gives.
    fn options(&self) -> Result<Options, Error> {
        let dpi = self
            .dpi
            .as_deref()
            .map(|d| d.parse().map_err(|_| Error::bad_dpi()));
        Options::parse(
            self.formats.as_deref(),
            self.pages.as_deref(),
            dpi.transpose()?,
            self.password.clone(),
            self.fast,
        )
    }
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    let bare = args.len() == 1;
    if bare && double_clicked() {
        let stop = format!(
            " {} close this window to stop.",
            Look::stdout().pick("·", "-")
        );
        return served(ui::serve(0, true, &stop));
    }
    // Only an exact first argument is a command: `./read` is a file.
    match args.get(1).and_then(|a| a.to_str()) {
        Some("read") => drop(args.remove(1)),
        Some("ui") => {
            let ui = Ui::parse_from(&args[1..]);
            return served(ui::serve(ui.port, !ui.no_open, "  (Ctrl+C to stop)"));
        }
        Some("mcp") => {
            Mcp::parse_from(&args[1..]);
            return served(mcp::serve());
        }
        _ => {}
    }
    let json = args
        .iter()
        .skip(1)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let parsed = Cli::command()
        .after_help(format!("{}\n\n{EXAMPLES}", formats()))
        .try_get_matches_from(&args)
        .and_then(|m| Cli::from_arg_matches(&m));
    let cli = match parsed {
        Ok(cli) => cli,
        Err(e) if json && e.use_stderr() => return fail(&Error::usage(clap_message(&e)), true),
        Err(e) => e.exit(),
    };
    let options = match cli.options() {
        Ok(options) => options,
        Err(e) => return fail(&e, cli.json),
    };
    let interactive = bare && io::stdin().is_terminal() && io::stdout().is_terminal();
    if cli.paths.is_empty() && !interactive {
        return fail(&Error::no_input(), cli.json);
    }
    let mut out = Out::new(io::stdout().lock(), Look::stdout());
    let mut err = Out::new(io::stderr(), Look::stderr());
    let run = if interactive {
        let read = |paths, out: &mut _, err: &mut _| {
            let cli = Cli {
                paths,
                ..Cli::default()
            };
            read(&cli, &options, out, err).map(drop)
        };
        interactive::run(io::stdin().lock(), &mut out, &mut err, read).map(|()| 0)
    } else {
        read(&cli, &options, &mut out, &mut err)
    };
    finish(run, &mut err)
}

/// The exit code of a run: a reader that hung up on stdout or stderr ends it quietly with 0,
/// any other write error is `io`.
fn finish(run: io::Result<u8>, err: &mut Out<impl Write>) -> ExitCode {
    match run {
        Ok(code) => ExitCode::from(code),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            let e = Error::io(e);
            let _ = text::error(err, &e.message, e.hint);
            ExitCode::from(3)
        }
    }
}

/// Reads every input in order, printing text per file as it finishes or one JSON object at
/// the end, and gives the exit code.
fn read(
    cli: &Cli,
    options: &Options,
    out: &mut Out<impl Write>,
    err: &mut Out<impl Write>,
) -> io::Result<u8> {
    let mut files = vec![];
    for input in input::expand(&cli.paths, cli.recursive, true) {
        let start = Instant::now();
        let name = Path::new(&input.name).file_name().unwrap_or_default();
        let file = text::spin(err, &name.to_string_lossy(), || {
            engine::read_input(&input, options)
        });
        if cli.debug {
            text::debug(err, &file, start.elapsed())?;
        }
        if !cli.json {
            text::file(out, err, &file)?;
        }
        files.push(file);
    }
    let report = Report::new(&files);
    if cli.json {
        serde_json::to_writer(&mut out.w, &report)?;
        writeln!(out.w)?;
    } else if files.len() > 1 {
        text::summary(out, &report)?;
    }
    out.w.flush()?;
    Ok(report.exit_code())
}

/// Whether Explorer started this process: it then has a console to itself, while a terminal
/// shares its console with the shell.
#[cfg(windows)]
fn double_clicked() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    let mut ids = [0u32; 2];
    // SAFETY: the call writes at most `ids.len()` process ids into `ids`.
    unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) == 1 }
}

#[cfg(not(windows))]
fn double_clicked() -> bool {
    false
}

/// How a server command ended: quietly, or with its error on stderr.
fn served(result: Result<(), Error>) -> ExitCode {
    result.map_or_else(|e| fail(&e, false), |()| ExitCode::SUCCESS)
}

/// Reports an error that stops the run, as two lines on stderr or as JSON on stdout, and
/// exits with its code.
fn fail(e: &Error, json: bool) -> ExitCode {
    let _ = if json {
        writeln!(io::stdout(), "{}", e.to_json())
    } else {
        text::error(
            &mut Out::new(io::stderr(), Look::stderr()),
            &e.message,
            e.hint,
        )
    };
    ExitCode::from(e.exit_code())
}

/// clap's message as a sentence: "unexpected argument '--x' found" -> "Unexpected argument
/// '--x' found."
fn clap_message(e: &clap::Error) -> String {
    let text = e.render().to_string();
    let line = text.lines().next().unwrap_or_default();
    let line = line.strip_prefix("error: ").unwrap_or(line);
    let mut chars = line.chars();
    let first = chars
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    format!("{first}{}.", chars.as_str())
}

/// The readable format names, wrapped to 80 columns, and the groups.
fn formats() -> String {
    let mut out = String::from("Formats (case, spaces, - _ and / don't matter):\n ");
    let mut width = 1;
    let names = BarcodeFormats::list(BarcodeFormat::AllReadable);
    let count = names.len();
    for (i, f) in names.iter().enumerate() {
        let word = format!(" {f}{}", if i + 1 < count { "," } else { "" });
        if width + word.len() > 80 {
            out += "\n ";
            width = 1;
        }
        width += word.len();
        out += &word;
    }
    out + "\n  Groups: all-linear, all-matrix, all-gs1, all-retail, all-industrial"
}
