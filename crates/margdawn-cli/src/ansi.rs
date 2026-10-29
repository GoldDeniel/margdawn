//! ANSI renderer: walks the parsed document tree and emits styled text.

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use margdawn_core::md::{Block, Document, Span};
use margdawn_core::theme::{Color as TColor, ColorKey, Theme};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const STRIKE: &str = "\x1b[9m";
const OSC8_OPEN: &str = "\x1b]8;;";
const OSC8_CLOSE: &str = "\x1b\\";

pub struct Options {
    pub color: bool,
    pub hyperlinks: bool,
    pub width: usize,
    pub preserve_blank_lines: bool,
}

pub fn detect_options() -> Options {
    let color = std::io::stdout().is_terminal();
    let hyperlinks = color && supports_osc8();
    let width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
        .max(40);
    Options {
        color,
        hyperlinks,
        width,
        preserve_blank_lines: true,
    }
}

fn supports_osc8() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    // broadly: terminals that report truecolor support OSC 8 too
    colorterm.contains("truecolor") || colorterm.contains("24bit") || term.contains("kitty") || term.contains("alacritty")
        || std::env::var("OCI_EXE").is_err() && term.contains("xterm-")
}

struct Ansi {
    color: bool,
    hyperlinks: bool,
}

impl Ansi {
    fn fg(&self, c: TColor) -> String {
        if !self.color {
            return String::new();
        }
        let truecolor = std::env::var("COLORTERM").is_ok_and(|v| v.contains("truecolor") || v.contains("24bit"));
        if truecolor {
            format!("\x1b[38;2;{}m", c.to_truecolor())
        } else {
            format!("\x1b[38;5;{}m", c.to_256color())
        }
    }

    fn link(&self, url: &str, text: &str) -> String {
        if self.hyperlinks && !url.is_empty() {
            format!("{OSC8_OPEN}{url}{OSC8_CLOSE}{text}{OSC8_OPEN}{OSC8_CLOSE}")
        } else if self.color && !url.is_empty() {
            format!("{text} {RESET}{}→ {url}{RESET}", self.fg(TColor::new(128, 128, 128)))
        } else {
            format!("{text} ({url})")
        }
    }
}

fn spans_to_line(out: &mut String, ansi: &Ansi, theme: &Theme, spans: &[Span], base: ColorKey) {
    let colors = &theme.colors;
    let mut emitted = false;
    for span in spans {
        let color = colors.resolve(span.color, base);
        if span.link.is_some() {
            // link styling handled per-span below
        }
        if span.text.is_empty() {
            continue;
        }
        let mut seq = String::new();
        if ansi.color {
            if span.bold {
                seq.push_str(BOLD);
            }
            if span.italic {
                seq.push_str(ITALIC);
            }
            if span.strike {
                seq.push_str(STRIKE);
            }
            seq.push_str(&ansi.fg(color));
        }
        let body = if let Some(url) = &span.link {
            ansi.link(url, &span.text)
        } else {
            span.text.clone()
        };
        if seq.is_empty() {
            out.push_str(&body);
        } else {
            out.push_str(&seq);
            out.push_str(&body);
            out.push_str(RESET);
        }
        emitted = true;
    }
    if !emitted {
        out.push('\n');
    }
}

fn indent(level: usize) -> String {
    "  ".repeat(level)
}

pub fn render<W: Write>(
    out: &mut W,
    doc: &Document,
    theme: Arc<Theme>,
    code_theme: &str,
    opts: &Options,
) -> std::io::Result<()> {
    let ansi = Ansi { color: opts.color, hyperlinks: opts.hyperlinks };
    let mut buf = String::new();
    for (i, db) in doc.blocks.iter().enumerate() {
        if i > 0 {
            if opts.preserve_blank_lines {
                // one paragraph break minimum + the author's blank lines
                for _ in 0..=db.blanks_before {
                    buf.push('\n');
                }
            } else {
                buf.push('\n');
            }
        }
        render_block(&mut buf, &ansi, &theme, code_theme, &db.block, 0, opts);
    }
    out.write_all(buf.as_bytes())?;
    Ok(())
}

fn render_block(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    code_theme: &str,
    block: &Block,
    depth: usize,
    opts: &Options,
) {
    let colors = &theme.colors;
    match block {
        Block::Heading(_level, spans) => {
            write_paragraph_bold(out, ansi, theme, spans, ColorKey::Heading, depth);
            out.push('\n');
        }
        Block::Paragraph(spans) => {
            write_paragraph(out, ansi, theme, spans, ColorKey::Foreground, depth);
            out.push('\n');
        }
        Block::Quote(depth, blocks) => {
            let bar = colors.resolve(ColorKey::Strong, ColorKey::Foreground);
            let bar = colors.quote_bar.unwrap_or(bar);
            for inner in blocks {
                let mut piece = String::new();
                if opts.color {
                    piece.push_str(&ansi.fg(bar));
                    piece.push_str("\u{2502} ");
                    piece.push_str(RESET);
                } else {
                    piece.push_str("| ");
                }
                out.push_str(&piece);
                render_block(out, ansi, theme, code_theme, inner, *depth as usize, opts);
            }
            out.push('\n');
        }
        Block::CodeBlock { lang, text } => {
            render_code_block(out, ansi, lang.as_deref(), text, code_theme, depth, opts.color);
        }
        Block::List { ordered, start, items } => {
            let marker_color = colors.resolve(ColorKey::Strong, ColorKey::Foreground);
            let marker_color = colors.list_marker.unwrap_or(marker_color);
            for (i, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    // stable per-list numbering via start offset
                    let n = start + i as u64;
                    format!("{n:>2}. ")
                } else {
                    "\u{2022} ".to_string()
                };
                let mut prefix = String::new();
                if opts.color {
                    prefix.push_str(&indent(depth));
                    prefix.push_str(&ansi.fg(marker_color));
                    prefix.push_str(&marker);
                    prefix.push_str(RESET);
                } else {
                    prefix.push_str(&indent(depth));
                    prefix.push_str(&marker);
                }
                if !prefix.is_empty() {
                    out.push_str(&prefix);
                }
                for inner in item {
                    // only indent continuation blocks, first block rides the marker
                    match inner {
                        Block::Paragraph(spans) => {
                            let mut piece = String::new();
                            write_paragraph_into(&mut piece, ansi, theme, spans, ColorKey::Foreground);
                            out.push_str(&piece);
                            out.push('\n');
                        }
                        other => {
                            out.push('\n');
                            render_block(out, ansi, theme, code_theme, other, depth + 1, opts);
                        }
                    }
                }
            }
            out.push('\n');
        }
        Block::Table { align: _, header, rows } => {
            render_table(out, ansi, theme, header, rows, depth, opts);
        }
        Block::Rule => {
            let hr_color = colors.resolve(ColorKey::Strike, ColorKey::Foreground);
            let hr_color = colors.hr.unwrap_or(hr_color);
            let w = opts.width.saturating_sub(2 * depth * 2).min(60);
            out.push_str(&indent(depth));
            if opts.color {
                out.push_str(&ansi.fg(hr_color));
            }
            for _ in 0..w {
                out.push('\u{2500}');
            }
            out.push_str(RESET);
            out.push('\n');
        }
        Block::FootnoteDef { label, blocks } => {
            let muted = colors.resolve(ColorKey::Strike, ColorKey::Foreground);
            out.push_str(&indent(depth));
            if opts.color {
                out.push_str(&ansi.fg(muted));
            }
            out.push_str(&format!("[{label}]"));
            out.push_str(RESET);
            out.push(' ');
            for inner in blocks {
                render_block(out, ansi, theme, code_theme, inner, depth, opts);
            }
            out.push('\n');
        }
    }
}

fn write_paragraph(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    spans: &[Span],
    base: ColorKey,
    depth: usize,
) {
    out.push_str(&indent(depth));
    let mut piece = String::new();
    write_paragraph_into(&mut piece, ansi, theme, spans, base);
    out.push_str(&piece);
}

fn write_paragraph_into(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    spans: &[Span],
    base: ColorKey,
) {
    spans_to_line(out, ansi, theme, spans, base);
}

fn write_paragraph_bold(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    spans: &[Span],
    base: ColorKey,
    depth: usize,
) {
    out.push_str(&indent(depth));
    let mut piece = String::new();
    spans_to_line_bold(&mut piece, ansi, theme, spans, base);
    out.push_str(&piece);
}

fn spans_to_line_bold(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    spans: &[Span],
    base: ColorKey,
) {
    if !ansi.color {
        spans_to_line(out, ansi, theme, spans, base);
        return;
    }
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let color = theme.colors.resolve(span.color, base);
        let mut seq = String::from(BOLD);
        if span.italic {
            seq.push_str(ITALIC);
        }
        if span.strike {
            seq.push_str(STRIKE);
        }
        seq.push_str(&ansi.fg(color));
        let body = match &span.link {
            Some(url) => ansi.link(url, &span.text),
            None => span.text.clone(),
        };
        out.push_str(&seq);
        out.push_str(&body);
        out.push_str(RESET);
    }
}

fn render_table(
    out: &mut String,
    ansi: &Ansi,
    theme: &Theme,
    header: &[Vec<Span>],
    rows: &[Vec<Vec<Span>>],
    depth: usize,
    opts: &Options,
) {
    let colors = &theme.colors;
    let border = colors.resolve(ColorKey::Strike, ColorKey::Foreground);
    let border = colors.table_border.unwrap_or(border);

    let cell_text = |cell: &[Span]| -> String { cell.iter().map(|s| s.text.as_str()).collect::<String>() };

    let mut widths = header.iter().map(|c| cell_text(c).chars().count()).collect::<Vec<_>>();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell_text(cell).chars().count());
            } else {
                widths.push(cell_text(cell).chars().count());
            }
        }
    }
    let pad = |s: &str, w: usize| {
        let len = s.chars().count();
        let mut out = String::from(s);
        for _ in len..w {
            out.push(' ');
        }
        out
    };

    let print_row = |out: &mut String, cells: &[Vec<Span>], bold_header: bool| {
        out.push_str(&indent(depth));
        if opts.color {
            out.push_str(&ansi.fg(border));
            out.push_str("\u{2502}");
            out.push_str(RESET);
        } else {
            out.push_str("|");
        }
        for (i, w) in widths.iter().enumerate() {
            let cell = cells.get(i);
            let text = cell.map(|c| cell_text(c)).unwrap_or_default();
            out.push(' ');
            if bold_header && opts.color {
                out.push_str(BOLD);
                out.push_str(&ansi.fg(colors.resolve(ColorKey::Heading, ColorKey::Heading)));
                out.push_str(&pad(&text, *w));
                out.push_str(RESET);
            } else if bold_header {
                out.push_str(&pad(&text, *w));
            } else if cell.is_some() {
                let mut piece = String::new();
                spans_to_line(&mut piece, ansi, theme, cell.unwrap(), ColorKey::Foreground);
                let first = piece.lines().next().unwrap_or_default();
                out.push_str(&pad(first, *w));
            } else {
                out.push_str(&pad("", *w));
            }
            out.push(' ');
            if opts.color {
                out.push_str(&ansi.fg(border));
                out.push_str("\u{2502}");
                out.push_str(RESET);
            } else {
                out.push_str("|");
            }
        }
        out.push('\n');
    };

    print_row(out, header, true);
    // separator
    out.push_str(&indent(depth));
    if opts.color {
        out.push_str(&ansi.fg(border));
    }
    out.push('\u{251C}');
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            out.push('\u{252C}');
        }
        for _ in 0..*w + 2 {
            out.push('\u{2500}');
        }
    }
    out.push('\u{2524}');
    out.push_str(RESET);
    out.push('\n');
    for row in rows {
        print_row(out, row, false);
    }
    out.push('\n');
}


/// Code blocks: syntax highlighted via syntect when color is on.
fn render_code_block(
    out: &mut String,
    ansi: &Ansi,
    lang: Option<&str>,
    text: &str,
    code_theme: &str,
    depth: usize,
    color: bool,
) {
    let highlighted = margdawn_core::syn::highlight(lang, text, code_theme);
    let fallback = highlighted.foreground;
    for runs in &highlighted.lines {
        out.push_str(&indent(depth + 1));
        if color {
            if runs.is_empty() {
                out.push_str(&ansi.fg(fallback));
            }
            for (c, text) in runs {
                out.push_str(&ansi.fg(*c));
                out.push_str(text);
                out.push_str(RESET);
            }
        } else {
            for (_, text) in runs {
                out.push_str(text);
            }
        }
        out.push('\n');
    }
}
