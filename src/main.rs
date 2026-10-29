//! The command line: `barcode-read [read] <PATH>... [OPTIONS]`.

mod text;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use barcode_read::engine::{self, Failure, Report};
use barcode_read::error::Error;
use barcode_read::input::{self, PageList};
use barcode_read::{Options, decode};
use clap::{ArgAction, CommandFactory, FromArgMatches, Parser};
use zxingcpp::{BarcodeFormat, BarcodeFormats};

const HELP: &str = "\
{about}

{usage-heading} {usage}

Commands:
  read  Read codes from files and folders (the default command)

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

#[derive(Parser)]
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
        let dpi = match &self.dpi {
            None => 300,
            Some(dpi) => dpi
                .parse()
                .ok()
                .filter(|dpi| (72..=1200).contains(dpi))
                .ok_or_else(Error::bad_dpi)?,
        };
        Ok(Options {
            formats: self
                .formats
                .as_deref()
                .map(decode::parse_formats)
                .transpose()?
                .unwrap_or_default(),
            fast: self.fast,
            dpi,
            pages: self.pages.as_deref().map(PageList::parse).transpose()?,
            password: self.password.clone(),
        })
    }
}

fn main() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    // Only an exact first argument is a command: `./read` is a file.
    if args.get(1).is_some_and(|a| a == "read") {
        args.remove(1);
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
        Err(e) if json && e.use_stderr() => return usage(&Error::usage(clap_message(&e)), true),
        Err(e) => e.exit(),
    };
    let options = match cli.options() {
        Ok(options) => options,
        Err(e) => return usage(&e, cli.json),
    };
    if cli.paths.is_empty() {
        return usage(&Error::no_input(), cli.json);
    }
    match read(&cli, &options) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            let _ = text::error(&mut io::stderr(), None, &Error::io(e));
            ExitCode::from(3)
        }
    }
}

/// Reads every input in order, printing text per file as it finishes or one JSON object at
/// the end, and gives the exit code.
fn read(cli: &Cli, options: &Options) -> io::Result<u8> {
    let (mut out, mut err) = (io::stdout().lock(), io::stderr());
    let mut files = vec![];
    for input in input::expand(&cli.paths, cli.recursive) {
        let file = engine::read_input(&input, options);
        if !cli.json {
            text::file(&mut out, &mut err, &file)?;
        }
        files.push(file);
    }
    let report = Report::new(&files);
    if cli.json {
        serde_json::to_writer(&mut out, &report)?;
        writeln!(out)?;
    } else if files.len() > 1 {
        text::summary(&mut out, &report)?;
    }
    out.flush()?;
    Ok(report.exit_code())
}

/// Reports a usage error, as two lines on stderr or as JSON on stdout, and exits 2.
fn usage(e: &Error, json: bool) -> ExitCode {
    let _ = if json {
        let failure = serde_json::to_string(&Failure { error: e }).expect("plain strings");
        writeln!(io::stdout(), "{failure}")
    } else {
        text::error(&mut io::stderr(), None, e)
    };
    ExitCode::from(2)
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
