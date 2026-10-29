//! Text output for people: codes and the no-code advice on stdout, warnings and errors on stderr.

use std::io::{self, Write};
use std::time::Duration;

use barcode_read::engine::{FileResult, Report};
use barcode_read::error::Error;
use barcode_read::input::Kind;
use base64::prelude::{BASE64_STANDARD, Engine};

/// One file's codes (`path  page N  format  tag`, then the text indented), then its warnings,
/// error or no-code line.
pub fn file(out: &mut impl Write, err: &mut impl Write, file: &FileResult) -> io::Result<()> {
    let paged = matches!(file.kind, Some(Kind::Pdf | Kind::Tiff));
    for c in &file.codes {
        let mut head = vec![file.path.clone()];
        if paged {
            head.push(format!("page {}", c.page));
        }
        head.push(c.format.clone());
        match (c.content, &c.bytes_base64) {
            ("gs1", _) => head.push("GS1".into()),
            (content, Some(bytes)) => {
                let n = BASE64_STANDARD.decode(bytes).map_or(0, |b| b.len());
                head.push(format!("{}, {n} bytes", content.replace('_', " ")));
            }
            _ => {}
        }
        writeln!(out, "{}", head.join("  "))?;
        for line in c.text.split('\n') {
            writeln!(out, "  {line}")?;
        }
    }
    for w in &file.warnings {
        if w.code == "none_found" {
            writeln!(out, "{}  no barcode found", file.path)?;
            if let Some(advice) = w.message.strip_prefix("No barcode found. ") {
                writeln!(out, "  {advice}")?;
            }
        } else {
            writeln!(err, "! {}: {}", file.path, w.message)?;
        }
    }
    match &file.error {
        Some(e) => error(err, Some(&file.path), e),
        None => Ok(()),
    }
}

/// `✗ path: message` (or just the message) and the hint indented on the next line.
pub fn error(err: &mut impl Write, path: Option<&str>, e: &Error) -> io::Result<()> {
    let path = path.map(|p| format!("{p}: ")).unwrap_or_default();
    writeln!(err, "✗ {path}{}", e.message)?;
    match e.hint {
        Some(hint) => writeln!(err, "  {hint}"),
        None => Ok(()),
    }
}

/// `3 codes in 2 of 4 files · 1 failed`.
pub fn summary(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let with_codes = report.files.iter().filter(|f| !f.codes.is_empty()).count();
    let plural = if report.found == 1 { "" } else { "s" };
    write!(
        out,
        "{} code{plural} in {with_codes} of {} files",
        report.found,
        report.files.len()
    )?;
    if report.failed > 0 {
        write!(out, " · {} failed", report.failed)?;
    }
    writeln!(out)
}

/// `--debug` lines: the type and time per file, each page's size and pass timings, and where a
/// caught panic happened.
pub fn debug(err: &mut impl Write, file: &FileResult, took: Duration) -> io::Result<()> {
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    let kind = file.kind.map_or("unknown type", Kind::name);
    writeln!(err, "debug: {}: {kind}, read in {}", file.path, ms(took))?;
    for p in &file.pages {
        let dpi = p.dpi.map(|d| format!(" at {d} dpi")).unwrap_or_default();
        let passes: Vec<_> = (1..)
            .zip(&p.passes)
            .map(|(i, d)| format!("pass {i} {}", ms(*d)))
            .collect();
        writeln!(
            err,
            "debug: {}: page {} {}x{}{dpi}: {}",
            file.path,
            p.page,
            p.width,
            p.height,
            passes.join(", ")
        )?;
    }
    if let Some(e) = &file.error {
        let at = e
            .cause
            .as_ref()
            .map(|c| format!(" (panicked at {c})"))
            .unwrap_or_default();
        writeln!(err, "debug: {}: error {}{at}", file.path, e.code)?;
    }
    Ok(())
}
