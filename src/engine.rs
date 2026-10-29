//! Reading one file end to end: its type, every selected page, the codes, warnings and error.
//! The decoding runs on a worker thread with a large stack, and a panic there becomes this
//! file's `internal` error, so the next file still reads.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::thread;
use std::time::Duration;

use crate::decode::{Code, Page, find_codes};
use crate::error::{Error, Warning, panic_message};
use crate::input::{self, Input, Kind};
use crate::{Options, pdf, raster, tiff};

/// The worker's name, which tells the panic hook to stay quiet.
const WORKER: &str = "reader";
const WORKER_STACK: usize = 64 << 20;

/// What reading one file gave.
#[derive(Debug)]
pub struct FileResult {
    pub path: String,
    /// `None` when the file couldn't be identified.
    pub kind: Option<Kind>,
    pub pages: Vec<PageInfo>,
    pub codes: Vec<Code>,
    pub warnings: Vec<Warning>,
    pub error: Option<Error>,
}

/// A page that was read, at its displayed size.
#[derive(Debug, Clone, PartialEq)]
pub struct PageInfo {
    pub page: usize,
    pub width: u32,
    pub height: u32,
    /// The resolution a PDF page was rendered at.
    pub dpi: Option<u32>,
    /// How long each decoding pass took.
    pub passes: Vec<Duration>,
}

/// Loads and reads one input.
pub fn read_input(input: &Input, options: &Options) -> FileResult {
    match input.load() {
        Ok(bytes) => read(input.name.clone(), bytes, options),
        Err(e) => FileResult {
            error: Some(e),
            ..FileResult::new(input.name.clone())
        },
    }
}

/// Reads a file's bytes; `path` is only reported. A file with an error has no codes.
pub fn read(path: String, bytes: Vec<u8>, options: &Options) -> FileResult {
    let mut file = FileResult::new(path);
    let read = input::sniff(&bytes).and_then(|kind| {
        file.kind = Some(kind);
        on_worker(|| pages(kind, bytes, options, &mut file))?
    });
    match read {
        Err(e) => {
            file.codes.clear();
            file.error = Some(e);
        }
        Ok(()) if file.codes.is_empty() => {
            let kind = file.kind.expect("identified").name();
            file.warnings.push(Warning::none_found(kind));
        }
        Ok(()) => {}
    }
    file
}

impl FileResult {
    fn new(path: String) -> Self {
        Self {
            path,
            kind: None,
            pages: vec![],
            codes: vec![],
            warnings: vec![],
            error: None,
        }
    }
}

fn pages(
    kind: Kind,
    bytes: Vec<u8>,
    options: &Options,
    file: &mut FileResult,
) -> Result<(), Error> {
    let mut failed = None;
    let mut each = |page: Page| {
        if failed.is_some() {
            return;
        }
        match find_codes(&page, options) {
            Ok(found) => {
                let (width, height) = page.size();
                file.pages.push(PageInfo {
                    page: page.number,
                    width,
                    height,
                    dpi: page.dpi,
                    passes: found.passes,
                });
                file.codes.extend(found.codes);
            }
            Err(e) => failed = Some(e),
        }
    };
    let warnings = &mut file.warnings;
    match kind {
        Kind::Pdf => pdf::pages(bytes, options, warnings, &mut each)?,
        Kind::Tiff => tiff::pages(&bytes, options, warnings, &mut each)?,
        kind => each(raster::image(kind, &bytes, warnings)?),
    }
    failed.map_or(Ok(()), Err)
}

thread_local! {
    /// Where the last panic on this thread happened.
    static PANIC_AT: Cell<Option<String>> = const { Cell::new(None) };
}

/// Runs `f` on a worker thread with a 64 MiB stack. A panic there prints nothing and becomes
/// the error `internal`, its location kept as the cause.
fn on_worker<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T, Error> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if thread::current().name() == Some(WORKER) {
                PANIC_AT.set(info.location().map(ToString::to_string));
            } else {
                default(info);
            }
        }));
    });
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name(WORKER.into())
            .stack_size(WORKER_STACK)
            .spawn_scoped(scope, || {
                catch_unwind(AssertUnwindSafe(f)).map_err(|panic| Error {
                    cause: PANIC_AT.take(),
                    ..Error::internal(panic_message(&*panic))
                })
            })
            .map_err(Error::internal)?;
        worker.join().expect("the worker catches its panics")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_is_an_internal_error_and_the_next_call_still_works() {
        let e = on_worker(|| -> u8 { panic!("boom") }).unwrap_err();
        assert_eq!(e.code, "internal");
        assert_eq!(
            e.message,
            "Something went wrong while reading this file: boom."
        );
        assert!(e.cause.as_ref().unwrap().contains("engine.rs"));
        assert_eq!(e.exit_code(), 3);
        assert_eq!(on_worker(|| 7), Ok(7));
    }

    #[test]
    fn deep_recursion_fits_the_worker_stack() {
        fn depth(n: u32) -> u32 {
            let pad = std::hint::black_box([0u8; 1024]);
            if n == 0 {
                pad[0].into()
            } else {
                depth(n - 1) + 1
            }
        }
        // About 30 MiB of frames: far past a default 2 MiB thread stack.
        assert_eq!(on_worker(|| depth(30_000)), Ok(30_000));
    }

    #[test]
    fn a_file_without_codes_gets_the_advice_for_its_type() {
        let pnm = b"P5\n8 8\n255\n".iter().copied().chain([255; 64]).collect();
        let file = read("blank.pgm".into(), pnm, &Options::default());
        assert_eq!(file.kind, Some(Kind::Pnm));
        assert_eq!(file.pages.len(), 1);
        assert_eq!(file.warnings, [Warning::none_found("pnm")]);
        let file = read("x".into(), b"hello".to_vec(), &Options::default());
        assert_eq!(
            (file.kind, file.error.unwrap().code),
            (None, "unsupported_format")
        );
        assert!(file.warnings.is_empty());
    }
}
