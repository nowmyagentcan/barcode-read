//! The reading engine shared by the CLI, the MCP server and the page. It never prints.

pub mod decode;
pub mod engine;
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
    /// The PDF and TIFF pages to read; `None` reads them all.
    pub pages: Option<input::PageList>,
    /// The password of an encrypted PDF.
    pub password: Option<String>,
}

impl Options {
    /// Options from the values people and agents type (`--formats`, `--pages`, ...), or the
    /// usage error the first bad one gives.
    pub fn parse(
        formats: Option<&str>,
        pages: Option<&str>,
        dpi: Option<u32>,
        password: Option<String>,
        fast: bool,
    ) -> Result<Self, error::Error> {
        Ok(Self {
            formats: formats
                .map(decode::parse_formats)
                .transpose()?
                .unwrap_or_default(),
            fast,
            dpi: match dpi {
                None => 300,
                Some(dpi) if (72..=1200).contains(&dpi) => dpi,
                Some(_) => return Err(error::Error::bad_dpi()),
            },
            pages: pages.map(input::PageList::parse).transpose()?,
            password,
        })
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            formats: vec![],
            fast: false,
            dpi: 300,
            pages: None,
            password: None,
        }
    }
}
