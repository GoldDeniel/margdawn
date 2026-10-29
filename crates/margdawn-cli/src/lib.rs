//! margdawn-cli: terminal rendering for margdawn.

pub mod ansi;

use std::io::{IsTerminal, Write};
use std::sync::Arc;

pub use margdawn_core::md::Document;
pub use margdawn_core::theme::Theme;

pub struct ViewOptions {
    /// Disable all ANSI styling. Defaults to true when stdout is not a TTY.
    pub ascii: bool,
    pub hyperlinks: bool,
    pub width: usize,
    /// Keep the document's blank lines between blocks.
    pub preserve_blank_lines: bool,
    /// Paint the document background (terminal).
    pub background: bool,
    /// Paint code-block panels and inline-code chips (terminal).
    pub code_background: bool,
}

impl ViewOptions {
    pub fn from_env() -> Self {
        let o = ansi::detect_options();
        Self {
            ascii: !std::io::stdout().is_terminal(),
            hyperlinks: o.hyperlinks,
            width: o.width,
            preserve_blank_lines: true,
            background: false,
            code_background: false,
        }
    }
}

pub fn render_to_stdout(
    doc: &Document,
    theme: Arc<Theme>,
    code_theme: &str,
    opts: &ViewOptions,
) -> std::io::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let a_opts = ansi::Options {
        color: !opts.ascii,
        hyperlinks: !opts.ascii && opts.hyperlinks,
        width: opts.width,
        preserve_blank_lines: opts.preserve_blank_lines,
        background: opts.background,
        code_background: opts.code_background,
    };
    ansi::render(&mut lock, doc, theme, code_theme, &a_opts)?;
    lock.flush()
}
