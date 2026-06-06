#![allow(clippy::module_inception)]

// Utility functions and helpers
//
// This module provides general-purpose utility functions including I/O operations,
// sorting, HTML rewriting, progress reporting, compression, and related helpers.

pub mod compression;
pub mod icu_wrapper;
pub mod io_utils;
pub mod mdx_html_rewriter;
pub mod progress_report;
pub mod sort_key;
pub mod url_utils;
pub mod utils;

pub use compression::{CompressionMethod, get_compressor};
pub use icu_wrapper::*;
pub use io_utils::{fix_windows_path_buf, read_exact_to_vec, scan_dir, windows_path_to_unix_path};
pub use mdx_html_rewriter::MdxHtmlRewriter;
pub use progress_report::{ProgressReportFn, ProgressState};
pub use sort_key::get_sort_key;
pub use url_utils::*;
pub use utils::{
    KeyComparable, RandomAccessable, binary_search_first, extract_text_from_html,
    html_escape_mdx_text, key_compare, locale_compare, move_element, remove_xml_declaration,
    sort_key_compare,
};
