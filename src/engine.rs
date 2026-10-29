//! Reading one file end to end: its type, every selected page, the codes, warnings and error.
//! The decoding runs on a worker thread with a large stack, and a panic there becomes this
//! file's `internal` error, so the next file still reads.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::thread;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Serialize;

use crate::decode::{Code, Page, find_codes};
use crate::error::{Error, Warning, panic_message};
use crate::input::{self, Input, Kind};
use crate::{Options, pdf, raster, tiff};

/// The worker's name, which tells the panic hook to stay quiet.
const WORKER: &str = "reader";
const WORKER_STACK: usize = 64 << 20;

/// What reading one file gave; it serialises as one entry of the JSON `files` list.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct FileResult {
    pub path: String,
    /// `None` when the file couldn't be identified.
    #[serde(rename = "type")]
    pub kind: Option<Kind>,
    pub pages: Vec<PageInfo>,
    #[serde(rename = "barcodes")]
    pub codes: Vec<Code>,
    pub warnings: Vec<Warning>,
    pub error: Option<Error>,
}

/// A page that was read, at its displayed size.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PageInfo {
    pub page: usize,
    pub width: u32,
    pub height: u32,
    /// The resolution a PDF page was rendered at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    /// How long each decoding pass took.
    #[serde(skip)]
    pub passes: Vec<Duration>,
}

/// The JSON result of a run: every file, the codes found in all of them, and the files that
/// failed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Report<'a> {
    pub files: &'a [FileResult],
    pub found: usize,
    pub failed: usize,
}

impl<'a> Report<'a> {
    pub fn new(files: &'a [FileResult]) -> Self {
        Self {
            files,
            found: files.iter().map(|f| f.codes.len()).sum(),
            failed: files.iter().filter(|f| f.error.is_some()).count(),
        }
    }

    /// 0 when a code was found and every file was read; 1 when nothing was found or a file
    /// failed; 3 when one hit an internal error. The highest applies.
    pub fn exit_code(&self) -> u8 {
        let errors = self.files.iter().filter_map(|f| f.error.as_ref());
        errors
            .map(Error::exit_code)
            .chain([u8::from(self.found == 0)])
            .max()
            .unwrap_or_default()
    }
}

/// The JSON for an error that stops a run, such as a bad option.
#[derive(Debug, Serialize)]
pub struct Failure<'a> {
    pub error: &'a Error,
}

/// Loads and reads one input.
pub fn read_input(input: &Input, options: &Options) -> FileResult {
    match input.load() {
        Ok(bytes) => read(input.name.clone(), bytes, options),
        Err(e) => FileResult {
            path: input.name.clone(),
            error: Some(e),
            ..Default::default()
        },
    }
}

/// Reads a file's bytes; `path` is only reported. A file with an error has no codes.
pub fn read(path: String, bytes: Vec<u8>, options: &Options) -> FileResult {
    let mut file = FileResult {
        path,
        ..Default::default()
    };
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
    fn the_json_keeps_the_documented_keys_and_order() {
        let qr = zxingcpp::create(zxingcpp::BarcodeFormat::QRCode)
            .from_str("hi")
            .unwrap()
            .to_image_with(&zxingcpp::write().scale(4))
            .unwrap();
        let (w, h) = (qr.width(), qr.height());
        let pgm = [format!("P5\n{w} {h}\n255\n").into_bytes(), qr.data()].concat();
        let files = [
            read("qr.pgm".into(), pgm, &Options::default()),
            read_input(
                &input::expand(&["gone.png".into()], false)[0],
                &Options::default(),
            ),
        ];
        let c = &files[0].codes[0];
        let (position, ec) = (
            serde_json::to_string(&c.position).unwrap(),
            c.ec_level.as_ref(),
        );
        let report = Report::new(&files);
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            format!(
                r#"{{"files":[{{"path":"qr.pgm","type":"pnm","pages":[{{"page":1,"width":{w},"height":{h}}}],"barcodes":[{{"page":1,"format":"QR Code","text":"hi","content":"text","bytes_base64":null,"symbology_id":"]Q1","position":{position},"rotation":0,"ec_level":"{}","version":"1","sequence":null}}],"warnings":[],"error":null}},{{"path":"gone.png","type":null,"pages":[],"barcodes":[],"warnings":[],"error":{{"code":"not_found","message":"File not found: gone.png","hint":"Check the path."}}}}],"found":1,"failed":1}}"#,
                ec.unwrap()
            )
        );
        assert_eq!(report.exit_code(), 1);
        assert_eq!(Report::new(&files[..1]).exit_code(), 0);
        let page = PageInfo {
            page: 2,
            width: 3,
            height: 4,
            dpi: Some(300),
            passes: vec![],
        };
        assert_eq!(
            serde_json::to_string(&page).unwrap(),
            r#"{"page":2,"width":3,"height":4,"dpi":300}"#
        );
        let e = Error::bad_dpi();
        assert_eq!(
            serde_json::to_string(&Failure { error: &e }).unwrap(),
            r#"{"error":{"code":"bad_dpi","message":"--dpi must be between 72 and 1200.","hint":null}}"#
        );
    }

    #[test]
    fn an_internal_error_outranks_the_others() {
        let file = |error: Option<Error>| FileResult {
            path: "x".into(),
            error,
            ..Default::default()
        };
        let files = [
            file(None),
            file(Some(Error::internal("boom"))),
            file(Some(Error::empty_file())),
        ];
        assert_eq!(Report::new(&files).exit_code(), 3);
        assert_eq!(Report::new(&[]).exit_code(), 1);
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
