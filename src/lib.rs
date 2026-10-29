//! The reading engine shared by the CLI, the MCP server and the page. It never prints.

pub mod decode;
pub mod error;
pub mod input;
pub mod pdf;
pub mod raster;
pub mod tiff;

use zxingcpp::BarcodeFormat;

/// How to read files; the default is what the CLI does without options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Formats and groups to look for; empty means every readable format.
    pub formats: Vec<BarcodeFormat>,
    /// One quick pass instead of up to three thorough ones.
    pub fast: bool,
    /// The resolution PDF pages are rendered at, 72 to 1200.
    pub dpi: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            formats: vec![],
            fast: false,
            dpi: 300,
        }
    }
}
