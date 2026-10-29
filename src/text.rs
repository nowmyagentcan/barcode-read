//! Text output for people: codes and the no-code advice on stdout, warnings and errors on
//! stderr. Colour only on a terminal without `NO_COLOR`; ASCII marks on a Windows console whose
//! output code page isn't UTF-8.

use std::env;
use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use barcode_read::engine::{FileResult, Report};
use barcode_read::input::Kind;
use base64::prelude::{BASE64_STANDARD, Engine};

pub const BOLD: &str = "1";
pub const DIM: &str = "2";
const RED: &str = "31";
const YELLOW: &str = "33";

/// How one stream shows text.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    /// A terminal that takes ANSI sequences.
    pub ansi: bool,
    pub colour: bool,
    pub ascii: bool,
}

impl Look {
    pub fn stdout() -> Self {
        Self::new(io::stdout().is_terminal(), console::STDOUT)
    }

    pub fn stderr() -> Self {
        Self::new(io::stderr().is_terminal(), console::STDERR)
    }

    fn new(terminal: bool, stream: console::Stream) -> Self {
        let ansi = terminal && console::enable_ansi(stream);
        Self {
            ansi,
            colour: ansi && env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
            ascii: terminal && !console::utf8(),
        }
    }

    /// `s` in the SGR style `sgr` when colour is on.
    pub fn paint(self, sgr: &str, s: &str) -> String {
        if self.colour {
            format!("\x1b[{sgr}m{s}\x1b[0m")
        } else {
            s.into()
        }
    }

    /// `unicode`, or `ascii` where the console can't show it.
    pub fn pick(self, unicode: &'static str, ascii: &'static str) -> &'static str {
        if self.ascii { ascii } else { unicode }
    }
}

#[cfg(windows)]
mod console {
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetConsoleOutputCP, GetStdHandle,
        STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };

    pub type Stream = STD_HANDLE;
    pub const STDOUT: Stream = STD_OUTPUT_HANDLE;
    pub const STDERR: Stream = STD_ERROR_HANDLE;

    /// Turns on ANSI sequences for the console behind `stream`.
    pub fn enable_ansi(stream: Stream) -> bool {
        // SAFETY: plain console calls on a standard handle; `mode` outlives the call.
        unsafe {
            let handle = GetStdHandle(stream);
            let mut mode = 0;
            GetConsoleMode(handle, &mut mode) != 0
                && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        }
    }

    pub fn utf8() -> bool {
        // SAFETY: no arguments; returns 0 without a console.
        unsafe { GetConsoleOutputCP() == 65001 }
    }
}

#[cfg(not(windows))]
mod console {
    pub type Stream = ();
    pub const STDOUT: Stream = ();
    pub const STDERR: Stream = ();

    pub fn enable_ansi(_: Stream) -> bool {
        true
    }

    pub fn utf8() -> bool {
        true
    }
}

/// A stream, how it shows text, and the indent every line gets.
pub struct Out<W> {
    pub w: W,
    pub look: Look,
    pub indent: &'static str,
}

impl<W: Write> Out<W> {
    pub fn new(w: W, look: Look) -> Self {
        Self {
            w,
            look,
            indent: "",
        }
    }

    pub fn line(&mut self, s: &str) -> io::Result<()> {
        writeln!(self.w, "{}{s}", self.indent)
    }
}

/// One file's codes (`path  page N  format  tag`, then the text indented), then its warnings,
/// error or no-code line.
pub fn file(
    out: &mut Out<impl Write>,
    err: &mut Out<impl Write>,
    file: &FileResult,
) -> io::Result<()> {
    let look = out.look;
    let path = look.paint(BOLD, &file.path);
    let paged = matches!(file.kind, Some(Kind::Pdf | Kind::Tiff));
    for c in &file.codes {
        let mut head = vec![path.clone()];
        if paged {
            head.push(look.paint(DIM, &format!("page {}", c.page)));
        }
        head.push(c.format.clone());
        match (c.content, &c.bytes_base64) {
            ("gs1", _) => head.push(look.paint(DIM, "GS1")),
            (content, Some(bytes)) => {
                let n = BASE64_STANDARD.decode(bytes).map_or(0, |b| b.len());
                let tag = format!("{}, {n} bytes", content.replace('_', " "));
                head.push(look.paint(DIM, &tag));
            }
            _ => {}
        }
        out.line(&head.join("  "))?;
        for line in c.text.split('\n') {
            out.line(&format!("  {line}"))?;
        }
    }
    for w in &file.warnings {
        if w.code == "none_found" {
            out.line(&format!("{path}  no barcode found"))?;
            if let Some(advice) = w.message.strip_prefix("No barcode found. ") {
                out.line(&format!("  {}", look.paint(DIM, advice)))?;
            }
        } else {
            let mark = err.look.paint(YELLOW, "!");
            err.line(&format!("{mark} {}: {}", file.path, w.message))?;
        }
    }
    match &file.error {
        Some(e) => error(err, &format!("{}: {}", file.path, e.message), e.hint),
        None => Ok(()),
    }
}

/// `✗ message` and the hint indented on the next line.
pub fn error(err: &mut Out<impl Write>, message: &str, hint: Option<&str>) -> io::Result<()> {
    let mark = err.look.paint(RED, err.look.pick("✗", "x"));
    err.line(&format!("{mark} {message}"))?;
    match hint {
        Some(hint) => err.line(&format!("  {hint}")),
        None => Ok(()),
    }
}

/// `3 codes in 2 of 4 files · 1 failed`.
pub fn summary(out: &mut Out<impl Write>, report: &Report) -> io::Result<()> {
    let with_codes = report.files.iter().filter(|f| !f.codes.is_empty()).count();
    let plural = if report.found == 1 { "" } else { "s" };
    let mut line = format!(
        "{} code{plural} in {with_codes} of {} files",
        report.found,
        report.files.len()
    );
    if report.failed > 0 {
        let dot = out.look.pick("·", "-");
        line += &format!(" {dot} {} failed", report.failed);
    }
    let line = out.look.paint(DIM, &line);
    out.line(&line)
}

/// How long a file reads before the spinner shows, and how long each frame stays.
const SPIN_AFTER: Duration = Duration::from_millis(300);
const FRAME: Duration = Duration::from_millis(80);

/// Runs `work`; when it takes a while and stderr is a terminal, `⠋ Reading name` spins on
/// stderr meanwhile and is erased before this returns.
pub fn spin<T>(err: &Out<impl Write>, name: &str, work: impl FnOnce() -> T) -> T {
    if !err.look.ansi {
        return work();
    }
    let (stop, stopped) = mpsc::channel();
    let (look, indent) = (err.look, err.indent);
    thread::scope(|s| {
        s.spawn(|| {
            spinner(
                io::stderr(),
                look,
                &format!("{indent}{{}} Reading {name}"),
                stopped,
            )
        });
        let result = work();
        drop(stop);
        result
    })
}

/// Draws `line` (its `{}` the frame) until `stopped` ends, then erases it.
fn spinner(mut w: impl Write, look: Look, line: &str, stopped: Receiver<()>) {
    let frames: &[&str] = if look.ascii {
        &["-", "\\", "|", "/"]
    } else {
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    };
    let mut drawn = false;
    for (i, frame) in frames.iter().cycle().enumerate() {
        let wait = if i == 0 { SPIN_AFTER } else { FRAME };
        if stopped.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
            break;
        }
        let text = look.paint(DIM, &line.replacen("{}", frame, 1));
        if write!(w, "\r{text}").and_then(|()| w.flush()).is_err() {
            return;
        }
        drawn = true;
    }
    if drawn {
        let _ = write!(w, "\r\x1b[K");
    }
}

/// `--debug` lines: the type and time per file, each page's size and pass timings, and where a
/// caught panic happened.
pub fn debug(err: &mut Out<impl Write>, file: &FileResult, took: Duration) -> io::Result<()> {
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    let kind = file.kind.map_or("unknown type", Kind::name);
    err.line(&format!(
        "debug: {}: {kind}, read in {}",
        file.path,
        ms(took)
    ))?;
    for p in &file.pages {
        let dpi = p.dpi.map(|d| format!(" at {d} dpi")).unwrap_or_default();
        let passes: Vec<_> = (1..)
            .zip(&p.passes)
            .map(|(i, d)| format!("pass {i} {}", ms(*d)))
            .collect();
        err.line(&format!(
            "debug: {}: page {} {}x{}{dpi}: {}",
            file.path,
            p.page,
            p.width,
            p.height,
            passes.join(", ")
        ))?;
    }
    if let Some(e) = &file.error {
        let at = e
            .cause
            .as_ref()
            .map(|c| format!(" (panicked at {c})"))
            .unwrap_or_default();
        err.line(&format!("debug: {}: error {}{at}", file.path, e.code))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use barcode_read::Options;
    use barcode_read::engine;

    const COLOUR: Look = Look {
        ansi: true,
        colour: true,
        ascii: false,
    };
    const ASCII: Look = Look {
        ansi: true,
        colour: false,
        ascii: true,
    };

    /// What `file` and `summary` print for a missing file and a blank image, on stdout and
    /// stderr.
    fn printed(look: Look) -> (String, String) {
        let blank = b"P5\n8 8\n255\n".iter().copied().chain([255; 64]).collect();
        let files = [
            engine::read("blank.pgm".into(), blank, &Options::default()),
            engine::read("x.png".into(), vec![], &Options::default()),
        ];
        let (mut out, mut err) = (Out::new(vec![], look), Out::new(vec![], look));
        for f in &files {
            file(&mut out, &mut err, f).unwrap();
        }
        summary(&mut out, &Report::new(&files)).unwrap();
        let text = |w: Vec<u8>| String::from_utf8(w).unwrap();
        (text(out.w), text(err.w))
    }

    #[test]
    fn colour_marks_paths_advice_and_errors() {
        let (out, err) = printed(COLOUR);
        assert_eq!(
            out,
            "\x1b[1mblank.pgm\x1b[0m  no barcode found\n  \x1b[2mTry a sharper, closer photo \
             with the whole code in view.\x1b[0m\n\x1b[2m0 codes in 0 of 2 files · 1 failed\x1b[0m\n"
        );
        assert_eq!(err, "\x1b[31m✗\x1b[0m x.png: The file is empty.\n");
    }

    #[test]
    fn plain_and_ascii_output_has_no_escape_sequences() {
        let (out, err) = printed(Look::default());
        assert!(out.ends_with("0 codes in 0 of 2 files · 1 failed\n"));
        assert_eq!(err, "✗ x.png: The file is empty.\n");
        let (out, err) = printed(ASCII);
        assert!(out.ends_with("0 codes in 0 of 2 files - 1 failed\n"));
        assert_eq!(err, "x x.png: The file is empty.\n");
        assert!(out.is_ascii() && !out.contains('\x1b'));
    }

    #[test]
    fn an_indent_starts_every_line() {
        let mut err = Out::new(vec![], Look::default());
        err.indent = "  ";
        error(&mut err, "Unknown command '/x'.", Some("Try /help.")).unwrap();
        assert_eq!(
            String::from_utf8(err.w).unwrap(),
            "  ✗ Unknown command '/x'.\n    Try /help.\n"
        );
    }

    #[test]
    fn the_spinner_shows_only_after_a_while_and_erases_itself() {
        let (stop, stopped) = mpsc::channel::<()>();
        drop(stop);
        let mut quick = vec![];
        spinner(&mut quick, COLOUR, "{} Reading a.png", stopped);
        assert!(quick.is_empty());

        let (stop, stopped) = mpsc::channel::<()>();
        let slow = thread::spawn(move || {
            let mut w = vec![];
            spinner(&mut w, ASCII, "  {} Reading scan.pdf", stopped);
            String::from_utf8(w).unwrap()
        });
        thread::sleep(Duration::from_secs(1));
        drop(stop);
        let slow = slow.join().unwrap();
        assert!(slow.starts_with("\r  - Reading scan.pdf\r  \\ Reading scan.pdf\r"));
        assert!(slow.ends_with("\r\x1b[K"));
    }
}
