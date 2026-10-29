//! margdawn-core: shared markdown parsing, theming, and configuration.

pub mod color;
pub mod config;
pub mod importer;
pub mod md;
pub mod syn;
pub mod theme;

pub use md::{parse, Block, Document, Span};
