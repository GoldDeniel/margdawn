//! ANSI renderer for the terminal, modelled on how agent CLIs render markdown
//! (glamour / Charm): a 2-column document margin, exactly one blank line
//! between blocks, greedy word wrapping with hanging indents for lists and
//! block quotes, inline-code chips, padded code panels and boxed tables.

use std::io::{IsTerminal, Write};
use std::sync::Arc;

use margdawn_core::md::{Block, CellAlign, Document, Span};
use margdawn_core::syn;
use margdawn_core::theme::{Color, ColorKey, Theme};
use unicode_width::UnicodeWidthStr;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const UNDERLINE: &str = "\x1b[4m";
const STRIKE: &str = "\x1b[9m";
const OSC8_OPEN: &str = "\x1b]8;;";
const OSC8_CLOSE: &str = "\x1b\\";
const NBSP: char = '\u{00A0}';

/// Left/right document margin (glamour uses 2).
const MARGIN: usize = 2;
/// Extra indentation for code blocks relative to the surrounding text.
const CODE_INDENT: usize = 2;

pub struct Options {
    pub color: bool,
    pub hyperlinks: bool,
    pub width: usize,
    /// Kept for config compatibility: the terminal renderer always uses a
    /// single blank line between blocks for a consistent rhythm.
    pub preserve_blank_lines: bool,
    /// Paint the document background across the terminal width.
    pub background: bool,
    /// Paint code-block panels and inline-code chips.
    pub code_background: bool,
}

pub fn detect_options() -> Options {
    let color = std::io::stdout().is_terminal();
    let hyperlinks = color && supports_osc8();
    let width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
        .max(20);
    Options {
        color,
        hyperlinks,
        width,
        preserve_blank_lines: true,
        background: false,
        code_background: false,
    }
}

fn supports_osc8() -> bool {
    let env = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
    let (colorterm, term, program) = (env("COLORTERM"), env("TERM"), env("TERM_PROGRAM"));
    colorterm.contains("truecolor")
        || colorterm.contains("24bit")
        || [
            "kitty",
            "alacritty",
            "wezterm",
            "ghostty",
            "foot",
            "vte",
            "iterm",
            "konsole",
            "xterm",
        ]
        .iter()
        .any(|t| term.contains(t) || program.contains(t))
}

// ---------------------------------------------------------------------------
// styled primitives

#[derive(Clone, Default, PartialEq, Eq)]
struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    link: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Seg {
    text: String,
    style: Style,
}

impl Seg {
    fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style }
    }

    fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

fn plain(text: impl Into<String>) -> Seg {
    Seg::new(text, Style::default())
}

fn segs_width(segs: &[Seg]) -> usize {
    segs.iter().map(Seg::width).sum()
}

fn merge_segs(segs: &mut Vec<Seg>) {
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for seg in segs.drain(..) {
        if seg.text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.style == seg.style {
                last.text.push_str(&seg.text);
                continue;
            }
        }
        out.push(seg);
    }
    *segs = out;
}

/// A horizontal offset: either plain spaces (list/code indentation) or a
/// block-quote bar. Order matters — `Spaces` then `Bar` is a quote inside a
/// list; `Bar` then `Spaces` is a list inside a quote.
#[derive(Clone, PartialEq, Eq)]
enum Lay {
    Spaces(usize),
    Bar(Seg),
}

impl Lay {
    fn width(&self) -> usize {
        match self {
            Lay::Spaces(n) => *n,
            Lay::Bar(seg) => seg.width(),
        }
    }
}

/// One output line: ordered indentation layers plus styled content.
#[derive(Clone, Default)]
struct Line {
    layers: Vec<Lay>,
    segs: Vec<Seg>,
}

/// Rendering context: the inherited indentation layers.
#[derive(Clone, Default)]
struct Ctx {
    layers: Vec<Lay>,
}

impl Ctx {
    fn width(&self) -> usize {
        self.layers.iter().map(Lay::width).sum()
    }

    /// Column at which content starts (document margin + layers).
    fn col(&self) -> usize {
        MARGIN + self.width()
    }

    fn with_spaces(&self, extra: usize) -> Ctx {
        let mut layers = self.layers.clone();
        if extra > 0 {
            layers.push(Lay::Spaces(extra));
        }
        Ctx { layers }
    }

    fn with_bar(&self, seg: Seg) -> Ctx {
        let mut layers = self.layers.clone();
        layers.push(Lay::Bar(seg));
        Ctx { layers }
    }
}

/// A line whose content starts at the context's indentation.
fn line_at(ctx: &Ctx, segs: Vec<Seg>) -> Line {
    Line { layers: ctx.layers.clone(), segs }
}

enum Unit {
    Word(Seg),
    Space,
    Break,
}

// ---------------------------------------------------------------------------
// renderer

struct Renderer<'a> {
    theme: &'a Theme,
    code_theme: &'a str,
    width: usize,
    color: bool,
    hyperlinks: bool,
    truecolor: bool,
    background: bool,
    code_background: bool,
}

pub fn render<W: Write>(
    out: &mut W,
    doc: &Document,
    theme: Arc<Theme>,
    code_theme: &str,
    opts: &Options,
) -> std::io::Result<()> {
    let truecolor = std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false);
    let renderer = Renderer {
        theme: &theme,
        code_theme,
        width: opts.width.max(20),
        color: opts.color,
        hyperlinks: opts.hyperlinks,
        truecolor,
        background: opts.background,
        code_background: opts.code_background,
    };
    let lines = renderer.document(doc);
    let mut s = String::new();
    renderer.emit(&lines, &mut s);
    out.write_all(s.as_bytes())
}

impl<'a> Renderer<'a> {
    fn document(&self, doc: &Document) -> Vec<Line> {
        let ctx = Ctx::default();
        let blocks: Vec<&Block> = doc.blocks.iter().map(|d| &d.block).collect();
        self.blocks(&blocks, &ctx, None)
    }

    fn blank(&self, ctx: &Ctx) -> Line {
        Line { layers: ctx.layers.clone(), segs: Vec::new() }
    }

    fn blocks(&self, blocks: &[&Block], ctx: &Ctx, muted: Option<Color>) -> Vec<Line> {
        let mut out = Vec::new();
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 {
                out.push(self.blank(ctx));
            }
            out.extend(self.block(b, ctx, muted));
        }
        out
    }

    fn block(&self, block: &Block, ctx: &Ctx, muted: Option<Color>) -> Vec<Line> {
        match block {
            Block::Paragraph(spans) => {
                self.wrap(spans, ctx, ColorKey::Foreground, muted, false)
            }
            Block::Heading(level, spans) => {
                // Without ANSI styling, mark headings so plain/piped output
                // stays scannable (like glamour's "notty" style).
                if self.color {
                    self.wrap(spans, ctx, ColorKey::Heading, muted, true)
                } else {
                    let mut spans = spans.clone();
                    spans.insert(
                        0,
                        Span {
                            text: format!("{} ", "#".repeat(*level as usize)),
                            bold: true,
                            italic: false,
                            strike: false,
                            code: false,
                            link: None,
                            color: ColorKey::Heading,
                        },
                    );
                    self.wrap(&spans, ctx, ColorKey::Heading, muted, true)
                }
            }
            Block::CodeBlock { lang, text } => self.code_block(lang.as_deref(), text, ctx),
            Block::Quote(_, blocks) => {
                let bar = self
                    .theme
                    .colors
                    .quote_bar
                    .unwrap_or_else(|| self.theme.colors.foreground());
                let quote_color = self
                    .theme
                    .colors
                    .quote
                    .unwrap_or_else(|| self.theme.colors.resolve(ColorKey::Muted, ColorKey::Foreground));
                let inner = ctx.with_bar(Seg::new(
                    "│ ",
                    Style { fg: Some(bar), ..Default::default() },
                ));
                let refs: Vec<&Block> = blocks.iter().collect();
                self.blocks(&refs, &inner, Some(muted.unwrap_or(quote_color)))
            }
            Block::List { ordered, start, items } => {
                self.list(*ordered, *start, items, ctx, muted)
            }
            Block::Table { align, header, rows } => self.table(align, header, rows, ctx),
            Block::Rule => self.rule(ctx),
            Block::FootnoteDef { label, blocks } => self.footnote(label, blocks, ctx, muted),
        }
    }

    // ---- inline -----------------------------------------------------------

    fn span_style(
        &self,
        span: &Span,
        base: ColorKey,
        muted: Option<Color>,
        bold_all: bool,
    ) -> Style {
        let colors = &self.theme.colors;
        let mut fg = colors.resolve(span.color, base);
        if span.color == ColorKey::Foreground {
            if let Some(m) = muted {
                fg = m;
            }
            if span.bold {
                fg = colors.strong.unwrap_or(fg);
            } else if span.italic {
                fg = colors.emphasis.unwrap_or(fg);
            }
        }
        let is_code = span.code && self.color;
        if is_code {
            fg = colors.resolve(ColorKey::Code, base);
        }
        Style {
            fg: Some(fg),
            bg: (is_code && self.code_background).then(|| colors.code_background()),
            bold: span.bold || bold_all,
            italic: span.italic,
            strike: span.strike,
            underline: span.link.is_some(),
            link: span.link.clone(),
        }
    }

    fn wrap(
        &self,
        spans: &[Span],
        ctx: &Ctx,
        base: ColorKey,
        muted: Option<Color>,
        bold_all: bool,
    ) -> Vec<Line> {
        let avail = self.width.saturating_sub(ctx.col()).max(16);
        let mut units = Vec::new();
        let muted_style = Style {
            fg: Some(self.theme.colors.resolve(ColorKey::Muted, ColorKey::Foreground)),
            ..Default::default()
        };

        for span in spans {
            let style = self.span_style(span, base, muted, bold_all);
            if span.code && self.color && self.code_background {
                // inline-code chip: non-breaking, padded with a thin space so
                // the background hugs the text (like glamour)
                let text = format!("{NBSP}{}{NBSP}", span.text);
                units.push(Unit::Word(Seg::new(text, style)));
                continue;
            }
            split_words(&span.text, &style, &mut units);
            if let Some(url) = &span.link {
                if !self.hyperlinks && is_external(url) {
                    units.push(Unit::Space);
                    units.push(Unit::Word(Seg::new(format!("({url})"), muted_style.clone())));
                }
            }
        }

        self.wrap_units(units, ctx, avail)
    }

    fn wrap_units(&self, units: Vec<Unit>, ctx: &Ctx, avail: usize) -> Vec<Line> {
        let mut lines: Vec<Line> = Vec::new();
        let mut cur: Vec<Seg> = Vec::new();
        let mut cur_w = 0usize;
        let mut pending_space = false;

        for unit in units {
            match unit {
                Unit::Break => {
                    flush(ctx, &mut lines, &mut cur, &mut cur_w);
                    pending_space = false;
                }
                Unit::Space => {
                    if !cur.is_empty() {
                        pending_space = true;
                    }
                }
                Unit::Word(seg) => push_word(
                    &mut cur,
                    &mut cur_w,
                    &mut pending_space,
                    seg,
                    avail,
                    ctx,
                    &mut lines,
                ),
            }
        }
        if !cur.is_empty() || lines.is_empty() {
            flush(ctx, &mut lines, &mut cur, &mut cur_w);
        }
        lines
    }

    // ---- blocks -----------------------------------------------------------

    fn code_block(&self, lang: Option<&str>, text: &str, ctx: &Ctx) -> Vec<Line> {
        let hl = syn::highlight(lang, text, self.code_theme);
        let inner = ctx.with_spaces(CODE_INDENT);
        // keep the same margin on the right as the panel's left inset
        let avail = self.width.saturating_sub(inner.col() * 2).max(8);
        let bg = hl.background;
        let mut lines = Vec::new();

        if !self.color || !self.code_background {
            for runs in &hl.lines {
                if self.color {
                    let mut segs = Vec::new();
                    for (color, t) in runs {
                        segs.push(Seg::new(
                            t.clone(),
                            Style { fg: Some(*color), ..Default::default() },
                        ));
                    }
                    lines.push(line_at(&inner, segs));
                } else {
                    let text: String = runs.iter().map(|(_, t)| t.as_str()).collect();
                    lines.push(line_at(&inner, vec![plain(text)]));
                }
            }
            if lines.is_empty() {
                lines.push(line_at(&inner, Vec::new()));
            }
            return lines;
        }

        // colored code panel
        let bg_style = Style { bg: Some(bg), ..Default::default() };
        for runs in &hl.lines {
            let mut segs = vec![Seg::new(" ", bg_style.clone())];
            let mut used = 1usize;
            for (color, t) in runs {
                used += UnicodeWidthStr::width(t.as_str());
                segs.push(Seg::new(
                    t.clone(),
                    Style { fg: Some(*color), bg: Some(bg), ..Default::default() },
                ));
            }
            if used < avail {
                segs.push(Seg::new(" ".repeat(avail - used), bg_style.clone()));
            }
            lines.push(line_at(&inner, segs));
        }
        if lines.is_empty() {
            lines.push(line_at(&inner, vec![Seg::new(" ".repeat(avail), bg_style)]));
        }
        lines
    }

    fn list(
        &self,
        ordered: bool,
        start: u64,
        items: &[Vec<Block>],
        ctx: &Ctx,
        muted: Option<Color>,
    ) -> Vec<Line> {
        let marker_color = self
            .theme
            .colors
            .list_marker
            .unwrap_or_else(|| self.theme.colors.foreground());
        let marker_style = Style { fg: Some(marker_color), ..Default::default() };
        let digits = if ordered {
            (start + items.len().saturating_sub(1) as u64).to_string().len()
        } else {
            1
        };

        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            // task list items replace the bullet with their checkbox
            let task = item.first().and_then(|b| match b {
                Block::Paragraph(spans) => strip_task_marker(spans),
                _ => None,
            });

            let marker_text = match &task {
                Some((ch, _)) => format!("{ch} "),
                None if ordered => format!("{:>width$}. ", start + i as u64, width = digits),
                None => "• ".to_string(),
            };
            let marker_w = UnicodeWidthStr::width(marker_text.as_str());
            let content = ctx.with_spaces(marker_w);
            let attach = matches!(item.first(), Some(Block::Paragraph(_)));

            let mut item_lines: Vec<Line> = Vec::new();
            for (j, child) in item.iter().enumerate() {
                // keep list items compact: no blank line right before a nested
                // list or right after one
                let skip_blank = j > 0
                    && (matches!(child, Block::List { .. })
                        || matches!(item[j - 1], Block::List { .. }));
                if j > 0 && !skip_blank {
                    item_lines.push(self.blank(&content));
                }

                let mut child_lines = match (j, &task) {
                    (0, Some((_, spans))) => {
                        self.wrap(spans, &content, ColorKey::Foreground, muted, false)
                    }
                    _ => self.block(child, &content, muted),
                };

                if j == 0 && attach {
                    if let Some(first) = child_lines.first_mut() {
                        first.layers = ctx.layers.clone();
                        let mut segs = vec![Seg::new(marker_text.clone(), marker_style.clone())];
                        segs.append(&mut first.segs);
                        first.segs = segs;
                    } else {
                        item_lines.push(line_at(
                            ctx,
                            vec![Seg::new(marker_text.clone(), marker_style.clone())],
                        ));
                    }
                }
                item_lines.extend(child_lines);
            }

            if !attach {
                out.push(line_at(ctx, vec![Seg::new(marker_text, marker_style.clone())]));
            }
            out.extend(item_lines);
        }
        out
    }

    fn table(
        &self,
        align: &[CellAlign],
        header: &[Vec<Span>],
        rows: &[Vec<Vec<Span>>],
        ctx: &Ctx,
    ) -> Vec<Line> {
        let cols = header
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1);

        // header row (bold) + body rows
        let mut all: Vec<(bool, Vec<Vec<Seg>>)> = Vec::new();
        if !header.is_empty() {
            let cells = (0..cols)
                .map(|c| header.get(c).map(|s| self.cell_segs(s, true)).unwrap_or_default())
                .collect();
            all.push((true, cells));
        }
        for row in rows {
            let cells = (0..cols)
                .map(|c| row.get(c).map(|s| self.cell_segs(s, false)).unwrap_or_default())
                .collect();
            all.push((false, cells));
        }
        if all.is_empty() {
            return Vec::new();
        }

        let mut widths = vec![1usize; cols];
        for (_, cells) in &all {
            for (c, segs) in cells.iter().enumerate() {
                widths[c] = widths[c].max(segs_width(segs));
            }
        }
        let avail = self.width.saturating_sub(ctx.col()).max(10);
        loop {
            let total: usize = widths.iter().sum::<usize>() + cols * 3 + 1;
            if total <= avail {
                break;
            }
            let Some((idx, w)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
                break;
            };
            if *w <= 3 {
                break;
            }
            widths[idx] -= 1;
        }

        let border_color = self
            .theme
            .colors
            .table_border
            .unwrap_or_else(|| self.neutral());
        let border = Style { fg: Some(border_color), ..Default::default() };
        let border_line = |left: &str, mid: &str, right: &str| -> Line {
            let mut segs = vec![Seg::new(left, border.clone())];
            for (i, w) in widths.iter().enumerate() {
                segs.push(Seg::new("─".repeat(w + 2), border.clone()));
                let ch = if i + 1 == cols { right } else { mid };
                segs.push(Seg::new(ch, border.clone()));
            }
            line_at(ctx, segs)
        };

        let mut out = vec![border_line("┌", "┬", "┐")];
        for (is_header, cells) in &all {
            let mut segs = vec![Seg::new("│", border.clone())];
            for (c, cell) in cells.iter().enumerate() {
                let a = align.get(c).copied().unwrap_or(CellAlign::Left);
                let (mut content, used) = truncate_segs(cell.clone(), widths[c]);
                let pad = widths[c].saturating_sub(used);
                let (lpad, rpad) = match a {
                    CellAlign::Left => (0, pad),
                    CellAlign::Center => (pad / 2, pad - pad / 2),
                    CellAlign::Right => (pad, 0),
                };
                segs.push(plain(" "));
                if lpad > 0 {
                    segs.push(plain(" ".repeat(lpad)));
                }
                segs.append(&mut content);
                if rpad > 0 {
                    segs.push(plain(" ".repeat(rpad)));
                }
                segs.push(plain(" "));
                segs.push(Seg::new("│", border.clone()));
            }
            out.push(line_at(ctx, segs));
            if *is_header {
                out.push(border_line("├", "┼", "┤"));
            }
        }
        out.push(border_line("└", "┴", "┘"));
        out
    }

    fn cell_segs(&self, spans: &[Span], header: bool) -> Vec<Seg> {
        let base = if header { ColorKey::Heading } else { ColorKey::Foreground };
        let mut segs = Vec::new();
        for span in spans {
            let style = self.span_style(span, base, None, header);
            segs.push(Seg::new(span.text.replace('\n', " "), style));
        }
        merge_segs(&mut segs);
        segs
    }

    fn rule(&self, ctx: &Ctx) -> Vec<Line> {
        let avail = self.width.saturating_sub(ctx.col()).max(10);
        let color = self.theme.colors.hr.unwrap_or_else(|| self.neutral());
        let style = Style { fg: Some(color), ..Default::default() };
        vec![line_at(ctx, vec![Seg::new("─".repeat(avail), style)])]
    }

    fn footnote(
        &self,
        label: &str,
        blocks: &[Block],
        ctx: &Ctx,
        muted: Option<Color>,
    ) -> Vec<Line> {
        let marker = format!("[{label}] ");
        let style = Style {
            fg: Some(self.theme.colors.resolve(ColorKey::Muted, ColorKey::Foreground)),
            ..Default::default()
        };
        let content = ctx.with_spaces(UnicodeWidthStr::width(marker.as_str()));
        let refs: Vec<&Block> = blocks.iter().collect();
        let mut lines = self.blocks(&refs, &content, muted);
        if let Some(first) = lines.first_mut() {
            first.layers = ctx.layers.clone();
            let mut segs = vec![Seg::new(marker, style)];
            segs.append(&mut first.segs);
            first.segs = segs;
            lines
        } else {
            vec![line_at(ctx, vec![Seg::new(marker, style)])]
        }
    }

    fn neutral(&self) -> Color {
        self.theme
            .colors
            .foreground()
            .blend(self.theme.colors.background(), 0.6)
    }

    // ---- serialization -----------------------------------------------------

    fn emit(&self, lines: &[Line], out: &mut String) {
        let mut rendered: Vec<String> = lines.iter().map(|l| self.line_string(l)).collect();
        while rendered.first().is_some_and(|s| s.trim().is_empty()) {
            rendered.remove(0);
        }
        while rendered.last().is_some_and(|s| s.trim().is_empty()) {
            rendered.pop();
        }
        // two lines of margin above and below the document
        for _ in 0..2 {
            rendered.insert(0, String::new());
            rendered.push(String::new());
        }
        if self.color && self.background {
            let bg = self.bg_seq(self.theme.colors.background());
            for line in &rendered {
                let used = visible_width(line);
                out.push_str(&bg);
                out.push_str(line);
                let pad = self.width.saturating_sub(used);
                if pad > 0 {
                    out.push_str(&" ".repeat(pad));
                }
                out.push_str(RESET);
                out.push('\n');
            }
        } else {
            for line in rendered {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }

    fn line_string(&self, line: &Line) -> String {
        if line.layers.is_empty() && line.segs.is_empty() {
            return String::new();
        }
        // container-only line (e.g. the bar between quoted paragraphs):
        // print the bars but no trailing padding
        let mut layers = line.layers.clone();
        if line.segs.is_empty() {
            while let Some(last) = layers.last_mut() {
                match last {
                    Lay::Spaces(_) => {
                        layers.pop();
                    }
                    Lay::Bar(seg) => {
                        let trimmed = seg.text.trim_end_matches(' ').len();
                        if trimmed == 0 {
                            layers.pop();
                        } else {
                            seg.text.truncate(trimmed);
                            break;
                        }
                    }
                }
            }
            if layers.is_empty() {
                return String::new();
            }
        }

        let mut s = " ".repeat(MARGIN);
        for lay in &layers {
            match lay {
                Lay::Spaces(n) => s.push_str(&" ".repeat(*n)),
                Lay::Bar(seg) => self.push_seg(&mut s, seg),
            }
        }
        if line.segs.is_empty() {
            return s;
        }
        let mut segs = line.segs.clone();
        // drop trailing spaces unless they are part of a background panel
        if segs.last().is_none_or(|seg| seg.style.bg.is_none()) {
            while let Some(last) = segs.last_mut() {
                let trimmed = last.text.trim_end_matches(' ').len();
                if trimmed == last.text.len() {
                    break;
                }
                last.text.truncate(trimmed);
                if last.text.is_empty() {
                    segs.pop();
                } else {
                    break;
                }
            }
        }
        for seg in &segs {
            self.push_seg(&mut s, seg);
        }
        s
    }

    fn push_seg(&self, out: &mut String, seg: &Seg) {
        if seg.text.is_empty() {
            return;
        }
        if !self.color {
            out.push_str(&seg.text);
            return;
        }
        let mut codes = String::new();
        if seg.style.bold {
            codes.push_str(BOLD);
        }
        if seg.style.italic {
            codes.push_str(ITALIC);
        }
        if seg.style.underline {
            codes.push_str(UNDERLINE);
        }
        if seg.style.strike {
            codes.push_str(STRIKE);
        }
        if let Some(c) = seg.style.fg {
            codes.push_str(&self.fg_seq(c));
        }
        if let Some(c) = seg.style.bg {
            codes.push_str(&self.bg_seq(c));
        }
        let osc = if self.hyperlinks {
            seg.style.link.as_deref().filter(|u| !u.is_empty())
        } else {
            None
        };
        if let Some(url) = osc {
            out.push_str(&format!("{OSC8_OPEN}{url}{OSC8_CLOSE}"));
        }
        out.push_str(&codes);
        out.push_str(&seg.text);
        out.push_str(RESET);
        if self.background {
            // re-apply the page background: a plain reset would make the
            // terminal revert to its own background mid-line
            out.push_str(&self.bg_seq(self.theme.colors.background()));
        }
        if osc.is_some() {
            out.push_str(&format!("{OSC8_OPEN}{OSC8_CLOSE}"));
        }
    }

    fn fg_seq(&self, c: Color) -> String {
        if self.truecolor {
            format!("\x1b[38;2;{}m", c.to_truecolor())
        } else {
            format!("\x1b[38;5;{}m", c.to_256color())
        }
    }

    fn bg_seq(&self, c: Color) -> String {
        if self.truecolor {
            format!("\x1b[48;2;{}m", c.to_truecolor())
        } else {
            format!("\x1b[48;5;{}m", c.to_256color())
        }
    }
}

// ---------------------------------------------------------------------------
// helpers

/// Display width of a string that may contain SGR/OSC sequences.
fn visible_width(s: &str) -> usize {
    let mut width = 0usize;
    let mut run = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // flush the plain run before skipping the escape
            width += UnicodeWidthStr::width(run.as_str());
            run.clear();
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ch.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        run.push(c);
    }
    width + UnicodeWidthStr::width(run.as_str())
}

fn is_external(url: &str) -> bool {
    url.contains("://") || url.starts_with("mailto:")
}

fn split_words(text: &str, style: &Style, out: &mut Vec<Unit>) {
    let mut word = String::new();
    let mut in_space = false;
    for ch in text.chars() {
        if ch == '\n' {
            if !word.is_empty() {
                out.push(Unit::Word(Seg::new(std::mem::take(&mut word), style.clone())));
            }
            out.push(Unit::Break);
            in_space = false;
        } else if ch.is_whitespace() && ch != NBSP {
            if !word.is_empty() {
                out.push(Unit::Word(Seg::new(std::mem::take(&mut word), style.clone())));
            }
            if !in_space {
                out.push(Unit::Space);
                in_space = true;
            }
        } else {
            in_space = false;
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Unit::Word(Seg::new(word, style.clone())));
    }
}

fn flush(ctx: &Ctx, lines: &mut Vec<Line>, cur: &mut Vec<Seg>, cur_w: &mut usize) {
    let mut segs = std::mem::take(cur);
    merge_segs(&mut segs);
    lines.push(line_at(ctx, segs));
    *cur_w = 0;
}

fn push_word(
    cur: &mut Vec<Seg>,
    cur_w: &mut usize,
    pending_space: &mut bool,
    seg: Seg,
    avail: usize,
    ctx: &Ctx,
    lines: &mut Vec<Line>,
) {
    let w = seg.width();
    let space_w = usize::from(*pending_space && !cur.is_empty());

    if !cur.is_empty() && *cur_w + space_w + w > avail {
        flush(ctx, lines, cur, cur_w);
        *pending_space = false;
    }

    if w > avail && cur.is_empty() {
        // break a too-long word across lines
        let pieces = split_seg(&seg, avail);
        let n = pieces.len();
        for (i, piece) in pieces.into_iter().enumerate() {
            if i + 1 == n {
                *cur_w = piece.width();
                cur.push(piece);
            } else {
                lines.push(line_at(ctx, vec![piece]));
            }
        }
        return;
    }

    if *pending_space && !cur.is_empty() {
        cur.push(plain(" "));
        *cur_w += 1;
        *pending_space = false;
    }
    *cur_w += w;
    cur.push(seg);
}

fn split_seg(seg: &Seg, max_width: usize) -> Vec<Seg> {
    let mut pieces = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in seg.text.char_indices() {
        let end = idx + ch.len_utf8();
        if idx > start && UnicodeWidthStr::width(&seg.text[start..end]) > max_width {
            pieces.push(Seg::new(seg.text[start..idx].to_string(), seg.style.clone()));
            start = idx;
        }
    }
    if start < seg.text.len() {
        pieces.push(Seg::new(seg.text[start..].to_string(), seg.style.clone()));
    }
    pieces
}

fn truncate_segs(segs: Vec<Seg>, max: usize) -> (Vec<Seg>, usize) {
    let full = segs_width(&segs);
    let truncated = full > max;
    let budget = if truncated { max.saturating_sub(1) } else { max };
    let mut out: Vec<Seg> = Vec::new();
    let mut used = 0usize;

    'outer: for seg in segs {
        let mut accepted = 0usize; // bytes accepted from this segment
        for (idx, ch) in seg.text.char_indices() {
            let end = idx + ch.len_utf8();
            // measure the candidate prefix as a whole string so emoji
            // sequences (ZWJ, skin tones, flags, VS16) count correctly
            let w = used + UnicodeWidthStr::width(&seg.text[..end]);
            if w > budget {
                break;
            }
            accepted = end;
        }
        if accepted > 0 {
            used += UnicodeWidthStr::width(&seg.text[..accepted]);
            out.push(Seg::new(seg.text[..accepted].to_string(), seg.style));
        }
        if accepted < seg.text.len() {
            break 'outer;
        }
    }

    if truncated {
        out.push(plain("…"));
    }
    // drop trailing spaces, then report the real width for padding
    while let Some(last) = out.last_mut() {
        let trimmed = last.text.trim_end_matches(' ');
        if trimmed.len() == last.text.len() {
            break;
        }
        last.text.truncate(trimmed.len());
        if last.text.is_empty() {
            out.pop();
        } else {
            break;
        }
    }
    merge_segs(&mut out);
    let width = segs_width(&out).min(max);
    (out, width)
}

fn strip_task_marker(spans: &[Span]) -> Option<(char, Vec<Span>)> {
    let first = spans.first()?;
    let ch = first.text.chars().next()?;
    if ch != '☑' && ch != '☐' {
        return None;
    }
    let mut out = spans.to_vec();
    let rest = first.text.strip_prefix(ch).unwrap_or(&first.text);
    out[0].text = rest.strip_prefix(' ').unwrap_or(rest).to_string();
    if out[0].text.is_empty() {
        out.remove(0);
    }
    Some((ch, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use margdawn_core::md::parse;

    fn theme() -> Arc<Theme> {
        Arc::new(margdawn_core::theme::builtin("claude").unwrap())
    }

    fn plain(md: &str, width: usize) -> Vec<String> {
        let theme = theme();
        let r = Renderer {
            theme: &theme,
            code_theme: "monokai",
            width,
            color: false,
            hyperlinks: false,
            truecolor: false,
            background: false,
            code_background: false,
        };
        let doc = parse(md);
        let mut s = String::new();
        r.emit(&r.document(&doc), &mut s);
        s.lines().map(String::from).collect()
    }

    fn ansi(md: &str, width: usize, background: bool, code_background: bool) -> String {
        let theme = theme();
        let r = Renderer {
            theme: &theme,
            code_theme: "monokai",
            width,
            color: true,
            hyperlinks: false,
            truecolor: true,
            background,
            code_background,
        };
        let doc = parse(md);
        let mut s = String::new();
        r.emit(&r.document(&doc), &mut s);
        s
    }

    /// Content lines only (drops the two-line document margins).
    fn content(md: &str, width: usize) -> Vec<String> {
        let mut lines = plain(md, width);
        while lines.first().is_some_and(|l| l.is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines
    }

    fn page_bg() -> String {
        format!("\x1b[48;2;{}m", margdawn_core::theme::builtin("claude")
            .unwrap()
            .colors
            .background()
            .to_truecolor())
    }

    /// Assert every visible character of a line is drawn with the page bg.
    fn assert_page_bg_everywhere(line: &str, bg: &str) {
        let mut current_bg: Option<String> = None;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    let mut params = String::new();
                    for ch in chars.by_ref() {
                        if ch == 'm' {
                            break;
                        }
                        params.push(ch);
                    }
                    if params == "0" || params.is_empty() {
                        current_bg = None;
                    } else if params.starts_with("48;2;") || params.starts_with("48;5;") {
                        current_bg = Some(format!("\x1b[{params}m"));
                    }
                } else if chars.peek() == Some(&']') {
                    // OSC8 hyperlink: skip payload
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                continue;
            }
            assert!(
                current_bg.as_deref() == Some(bg),
                "visible char {c:?} without page background in {line:?}"
            );
        }
    }

    #[test]
    fn two_line_margins_top_and_bottom() {
        let lines = plain("# hi\n\ntext\n", 40);
        assert_eq!(&lines[..2], &["", ""], "two blank lines on top");
        assert_eq!(&lines[lines.len() - 2..], &["", ""], "two blank lines at the bottom");
        assert_eq!(lines[2], "  # hi");
    }

    #[test]
    fn one_blank_line_between_blocks() {
        let out = content("first\n\n\n\nsecond\n", 40);
        assert_eq!(out, vec!["  first", "", "  second"]);
    }

    #[test]
    fn paragraphs_wrap_with_margin() {
        let text = "word ".repeat(40);
        for line in content(&text, 30) {
            assert!(!line.is_empty());
            assert!(line.starts_with("  "), "margin missing: {line:?}");
            assert!(UnicodeWidthStr::width(line.as_str()) <= 30, "too wide: {line:?}");
        }
    }

    #[test]
    fn list_hanging_indent() {
        let out = content("- a fairly long list item that wraps over lines\n", 34);
        assert!(out[0].starts_with("  • a"), "{:?}", out[0]);
        assert!(out.len() > 1);
        for cont in &out[1..] {
            assert!(cont.starts_with("    "), "continuation not aligned: {cont:?}");
        }
    }

    #[test]
    fn quote_bar_on_every_line() {
        let out = content(
            "> a quoted paragraph long enough to wrap onto several lines for sure\n>\n> second paragraph\n",
            40,
        );
        for line in &out {
            assert!(
                line.is_empty() || line.starts_with("  │"),
                "quote line without bar: {line:?}"
            );
        }
        assert!(
            out.iter().any(|l| l == "  │"),
            "blank line between quote paragraphs should keep the bar"
        );
    }

    #[test]
    fn ordered_numbers_aligned() {
        let out = content("9. nine\n10. ten\n11. eleven\n", 40);
        assert!(out[0].starts_with("   9. nine"), "{:?}", out[0]);
        assert!(out[1].starts_with("  10. ten"), "{:?}", out[1]);
        assert!(out[2].starts_with("  11. eleven"), "{:?}", out[2]);
    }

    #[test]
    fn tables_fit_the_width() {
        let out = content("| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n", 40);
        for line in &out {
            assert!(line.starts_with("  "), "{line:?}");
            assert!(UnicodeWidthStr::width(line.as_str()) <= 40, "{line:?}");
        }
        assert!(out[0].starts_with("  ┌"));
        assert!(out.last().unwrap().starts_with("  └"));
    }

    #[test]
    fn code_backgrounds_off_by_default() {
        let out = ansi(
            "Text with `code` here.\n\n```rust\nfn main() {}\n```\n",
            80,
            false,
            false,
        );
        assert!(!out.contains("\x1b[48;2;"), "backgrounds should be off by default");
        assert!(out.contains("\x1b[38;2;"), "foreground colors should remain");
    }

    #[test]
    fn code_backgrounds_toggle_inline_and_blocks() {
        let out = ansi(
            "Text with `code` here.\n\n```rust\nfn main() {}\n```\n",
            80,
            false,
            true,
        );
        assert!(out.contains("\x1b[48;2;"), "expected code backgrounds");
        assert!(
            out.matches("\x1b[48;2;").count() > 1,
            "inline chip and block panel both need a background"
        );
    }

    #[test]
    fn document_background_is_optional_and_full_width() {
        let off = ansi("first\n\nsecond\n", 30, false, false);
        assert!(!off.contains("\x1b[48;2;"));

        let on = ansi("first\n\nsecond\n", 30, true, false);
        for line in on.lines() {
            assert!(line.starts_with("\x1b[48;2;"), "line lacks page bg: {line:?}");
            assert_eq!(visible_width(line), 30, "line not padded to width: {line:?}");
        }
    }

    #[test]
    fn page_background_survives_inline_resets() {
        let out = ansi(
            "Some **bold**, *italic*, `code` and a [link](https://example.com) here.\n\n> quoted text\n",
            48,
            true,
            false,
        );
        let bg = page_bg();
        for line in out.lines() {
            assert_page_bg_everywhere(line, &bg);
        }
    }

    #[test]
    fn code_panel_margins_are_symmetric() {
        let width = 60;
        let out = ansi("```rust\nfn main() {}\n```\n", width, false, true);
        let panel_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains("\x1b[48;2;39;40;34m"))
            .collect();
        assert!(!panel_lines.is_empty(), "expected monokai panel");
        for line in panel_lines {
            // panel starts at margin(2) + code indent(2) = 4 and keeps the
            // same 4 columns free on the right
            assert_eq!(visible_width(line), width - 4, "{line:?}");
        }
    }

    /// Display-column positions of table border glyphs on a line.
    fn border_positions(line: &str) -> Vec<usize> {
        let mut col = 0usize;
        let mut run = String::new();
        let mut out = Vec::new();
        for ch in line.chars() {
            match ch {
                '│' | '┌' | '┬' | '┐' | '├' | '┼' | '┤' | '└' | '┴' | '┘' => {
                    col += UnicodeWidthStr::width(run.as_str());
                    run.clear();
                    out.push(col);
                    col += 1;
                }
                _ => run.push(ch),
            }
        }
        out
    }

    #[test]
    fn emoji_table_columns_align() {
        let md = "| a | b | c |\n|:-:|:-:|:-:|\n\
                  | 😀 | 日本語 | x |\n\
                  | 👨‍👩‍👧 | 👍🏽 | 🇸🇪 |\n\
                  | ❤️ | 1️⃣ | a long long long long long cell value |\n";
        let out = content(md, 64);
        let positions: Vec<Vec<usize>> = out
            .iter()
            .filter(|l| l.contains(['│', '┌', '├', '└']))
            .map(|l| border_positions(l))
            .collect();
        assert!(positions.len() >= 7, "expected full table, got {out:?}");
        for p in &positions[1..] {
            assert_eq!(p, &positions[0], "borders drift between rows: {out:?}");
        }
    }

    #[test]
    fn truncation_uses_sequence_widths() {
        let segs = vec![Seg::new("👨‍👩‍👧", Style::default())];
        let (out, used) = truncate_segs(segs, 2);
        assert_eq!(used, 2, "ZWJ family must count as one 2-column glyph");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "👨‍👩‍👧");
        assert!(!out[0].text.contains('…'));
    }

    #[test]
    fn long_ascii_cells_are_truncated_with_ellipsis() {
        let segs = vec![Seg::new("abcdefghij", Style::default())];
        let (out, used) = truncate_segs(segs, 6);
        assert_eq!(used, 6);
        let text: String = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "abcde…");
    }
}
