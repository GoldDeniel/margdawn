//! Parse Markdown into a simple block tree that both renderers consume.
//!
//! The parser preserves two things minimal viewers usually trim away:
//! - the number of blank lines between top-level blocks (so the author's
//!   spacing survives rendering), and
//! - per-column alignment of tables.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

use crate::theme::ColorKey;

/// One inline run. `color` is the palette slot; flags modify style.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub strike: bool,
    #[serde(default)]
    pub code: bool,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub color: ColorKey,
}

/// Optional span-level background (used for inline code chips).
pub type SpanBg = Option<ColorKey>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Block {
    Paragraph(Vec<Span>),
    /// (level 1..=6, spans)
    Heading(u8, Vec<Span>),
    CodeBlock { lang: Option<String>, text: String },
    Quote(u8, Vec<Block>),
    List { ordered: bool, start: u64, items: Vec<Vec<Block>> },
    Table { align: Vec<CellAlign>, header: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
    FootnoteDef { label: String, blocks: Vec<Block> },
}

/// A top-level block with the number of blank lines that preceded it in the
/// source document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocBlock {
    #[serde(default)]
    pub blanks_before: u8,
    #[serde(flatten)]
    pub block: Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellAlign {
    /// steal default
    Left,
    Center,
    Right,
}

impl Default for CellAlign {
    fn default() -> Self {
        Self::Left
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub blocks: Vec<DocBlock>,
}

pub fn parse(src: &str) -> Document {
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    Builder::run(src, Parser::new_ext(src, options))
}

#[derive(Clone)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    color: ColorKey,
    link: Option<String>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            strike: false,
            color: ColorKey::Foreground,
            link: None,
        }
    }
}

/// Inline buffer: consecutive pushes with identical style merge.
#[derive(Default)]
struct Inline(Vec<Span>);

impl Inline {
    fn push(&mut self, text: &str, st: &Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.0.last_mut() {
            if last.bold == st.bold
                && last.italic == st.italic
                && last.strike == st.strike
                && last.code == false
                && last.color == st.color
                && last.link == st.link
            {
                last.text.push_str(text);
                return;
            }
        }
        self.0.push(Span {
            text: text.to_string(),
            bold: st.bold,
            italic: st.italic,
            strike: st.strike,
            code: false,
            link: st.link.clone(),
            color: st.color,
        });
    }

    fn take(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.0)
    }
}

enum Open {
    Quote { depth: u8, blocks: Vec<Block> },
    List { ordered: bool, start: u64, items: Vec<Vec<Block>> },
    Item(Vec<Block>),
    Footnote { label: String, blocks: Vec<Block> },
}

impl Open {
    fn blocks_mut(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Open::Quote { blocks, .. } => Some(blocks),
            Open::Item(blocks) => Some(blocks),
            Open::Footnote { blocks, .. } => Some(blocks),
            Open::List { .. } => None,
        }
    }
}

struct TableState {
    align: Vec<CellAlign>,
    header: Option<Vec<Vec<Span>>>,
    current_row: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
}

struct Builder<'src> {
    src: &'src str,
    root: Vec<DocBlock>,
    stack: Vec<Open>,
    inline: Inline,
    style: Style,
    heading: Option<u8>,
    code_text: Option<String>,
    code_lang: Option<String>,
    /// color to restore when a link closes
    link_prev_color: Option<ColorKey>,
    table: Option<TableState>,
    /// Byte offset where the last piece of inline content ended; gaps between
    /// blocks are measured from here to the start of the next block.
    last_content_end: usize,
    /// Blank-line count in front of the block currently being collected.
    next_gap: u8,
}

fn count_blanks(src: &str, from: usize, to: usize) -> u8 {
    if to <= from || to > src.len() {
        return 0;
    }
    let gap = src
        .get(from..to)
        .unwrap_or_default()
        .matches('\n')
        .count();
    gap.saturating_sub(1).min(u8::MAX as usize) as u8
}

impl<'src> Builder<'src> {
    fn run(src: &'src str, events: Parser<'src>) -> Document {
        let mut b = Builder {
            src,
            root: Vec::new(),
            stack: Vec::new(),
            inline: Inline::default(),
            style: Style::default(),
            heading: None,
            code_text: None,
            code_lang: None,
            link_prev_color: None,
            table: None,
            last_content_end: 0,
            next_gap: 0,
        };
        for (ev, range) in events.into_offset_iter() {
            b.event(ev, range.clone());
        }
        Document { blocks: b.root }
    }

    fn quote_depth(&self) -> u8 {
        self.stack
            .iter()
            .filter(|o| matches!(o, Open::Quote { .. }))
            .count() as u8
            + 1
    }

    fn event(&mut self, ev: Event<'_>, range: std::ops::Range<usize>) {
        match ev {
            Event::Start(tag) => self.start(tag, range.clone()),
            Event::End(tag_end) => self.end(tag_end, range),
            Event::Text(t) => {
                if let Some(buf) = &mut self.code_text {
                    buf.push_str(&t);
                } else {
                    self.inline.push(&t, &self.style);
                }
                self.last_content_end = range.end;
            }
            Event::Code(t) => {
                self.last_content_end = range.end;
                self.push_code_span(&t);
            }
            Event::SoftBreak => {
                if let Some(buf) = &mut self.code_text {
                    buf.push('\n');
                } else {
                    self.inline.push("\n", &self.style);
                }
            }
            Event::HardBreak => self.inline.push("\n", &self.style),
            Event::Rule => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.last_content_end = range.end;
                let gap = self.next_gap;
                self.next_gap = 0;
                self.root.push(DocBlock {
                    blanks_before: gap,
                    block: Block::Rule,
                });
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "\u{2611} " } else { "\u{2610} " };
                self.inline.push(marker, &self.style);
            }
            Event::FootnoteReference(n) => {
                self.inline.push(&format!("[{n}]"), &self.style);
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {}
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: std::ops::Range<usize>) {
        match tag {
            Tag::Paragraph => {
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.heading = Some(level_to_u8(level));
                self.style.color = ColorKey::Heading;
            }
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => {
                self.link_prev_color = Some(self.style.color);
                self.style.color = ColorKey::Link;
                self.style.link = Some(dest_url.to_string());
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.code_text = Some(String::new());
                self.code_lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|l| !l.is_empty())
                        .map(String::from),
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                let depth = self.quote_depth();
                self.stack.push(Open::Quote { depth, blocks: Vec::new() });
            }
            Tag::List(start) => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.stack.push(Open::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                // Tight lists emit raw text (no Paragraph tags), so flush any
                // pending inline into the previous sibling or open container.
                self.flush_inline();
                self.stack.push(Open::Item(Vec::new()));
            }
            Tag::FootnoteDefinition(label) => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.stack.push(Open::Footnote {
                    label: label.to_string(),
                    blocks: Vec::new(),
                });
            }
            Tag::Table(alignments) => {
                self.flush_inline();
                self.next_gap = count_blanks(self.src, self.last_content_end, range.start);
                self.table = Some(TableState {
                    align: alignments.iter().map(|a| match a {
                        pulldown_cmark::Alignment::Center => CellAlign::Center,
                        pulldown_cmark::Alignment::Right => CellAlign::Right,
                        _ => CellAlign::Left,
                    }).collect(),
                    header: None,
                    current_row: Vec::new(),
                    rows: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                // start a fresh cell buffer
                self.inline.take();
            }
            Tag::Image { dest_url, .. } => {
                self.inline.push("\u{1F5BC} ", &self.style);
                self.style.link = Some(dest_url.to_string());
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => {}
        }
    }

    fn end(&mut self, tag: TagEnd, _range: std::ops::Range<usize>) {
        match tag {
            TagEnd::Paragraph => self.flush_inline(),
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let spans = self.inline.take();
                self.style.color = ColorKey::Foreground;
                self.push_top(Block::Heading(level, spans));
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => {
                self.style.link = None;
                if let Some(c) = self.link_prev_color.take() {
                    self.style.color = c;
                }
            }
            TagEnd::Image => self.style.link = None,
            TagEnd::CodeBlock => {
                if let Some(text) = self.code_text.take() {
                    let lang = self.code_lang.take();
                    self.push_top(Block::CodeBlock { lang, text });
                }
            }
            TagEnd::BlockQuote => {
                if let Some(Open::Quote { depth, blocks }) = self.stack.pop() {
                    self.push_top(Block::Quote(depth, blocks));
                }
            }
            TagEnd::List(_) => {
                if let Some(Open::List { ordered, start, items }) = self.stack.pop() {
                    self.push_top(Block::List { ordered, start, items });
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                let blocks = match self.stack.pop() {
                    Some(Open::Item(blocks)) => blocks,
                    other => {
                        if let Some(o) = other {
                            self.stack.push(o);
                        }
                        return;
                    }
                };
                if let Some(Open::List { items, .. }) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|o| matches!(o, Open::List { .. }))
                {
                    items.push(blocks);
                } else {
                    for block in blocks {
                        self.root.push(DocBlock { blanks_before: 0, block });
                    }
                }
            }
            TagEnd::FootnoteDefinition => {
                if let Some(Open::Footnote { label, blocks }) = self.stack.pop() {
                    self.push_top(Block::FootnoteDef { label, blocks });
                }
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.push_top(Block::Table {
                        align: t.align,
                        header: t.header.unwrap_or_default(),
                        rows: t.rows,
                    });
                }
            }
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.header = Some(std::mem::take(&mut t.current_row));
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table {
                    t.rows.push(std::mem::take(&mut t.current_row));
                }
            }
            TagEnd::TableCell => {
                let spans = self.inline.take();
                if let Some(t) = &mut self.table {
                    t.current_row.push(spans);
                }
            }
            _ => {}
        }
    }

    fn push_code_span(&mut self, text: &str) {
        let st = self.style.clone();
        self.inline.0.push(Span {
            text: text.to_string(),
            bold: st.bold,
            italic: st.italic,
            strike: st.strike,
            code: true,
            link: st.link.clone(),
            color: ColorKey::Code,
        });
    }

    /// Flush pending inline as a paragraph/routed block.
    fn flush_inline(&mut self) {
        let spans = self.inline.take();
        if spans.is_empty() {
            return;
        }
        // Inside a container the pending paragraph belongs to that container
        // and carries no gap; gaps only apply at the top level.
        if let Some(open) = self.stack.last_mut() {
            if let Some(buf) = open.blocks_mut() {
                buf.push(Block::Paragraph(spans));
                return;
            }
        }
        if self.table.is_some() {
            // table cells flush via End(TableCell)
            return;
        }
        self.push_top(Block::Paragraph(spans));
    }

    /// Push a completed block: into innermost open container when nested,
    /// otherwise into the top-level list with the tracked blank-line gap.
    fn push_top(&mut self, block: Block) {
        if let Some(open) = self.stack.last_mut() {
            if let Some(buf) = open.blocks_mut() {
                buf.push(block);
                return;
            }
        }
        let gap = self.next_gap;
        self.next_gap = 0;
        self.root.push(DocBlock {
            blanks_before: gap,
            block,
        });
    }
}

fn level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(doc: &Document) -> Vec<&'static str> {
        doc.blocks
            .iter()
            .map(|db| match &db.block {
                Block::Heading(..) => "h",
                Block::Paragraph(..) => "p",
                Block::List { .. } => "list",
                Block::Quote(..) => "quote",
                Block::CodeBlock { .. } => "code",
                Block::Rule => "rule",
                Block::Table { .. } => "table",
                Block::FootnoteDef { .. } => "fn",
            })
            .collect()
    }

    #[test]
    fn basic_document() {
        let doc = parse("# Hello\n\nWorld **bold** *italic* ~~gone~~ `code` and [link](https://x.y)\n\n- a\n- b\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n---\n");
        assert_eq!(kinds(&doc), vec!["h", "p", "list", "quote", "code", "rule"]);
    }

    #[test]
    fn preserves_blank_lines_and_table_alignment() {
        let doc = parse("uno\n\n\n\ndue\n\n| a | b |\n|:-:|--:|\n| 1 | 2 |\n\n\ntre\n");
        let gaps: Vec<u8> = doc.blocks.iter().map(|b| b.blanks_before).collect();
        assert_eq!(gaps, vec![0, 3, 1, 2]);
        match &doc.blocks[2].block {
            Block::Table { align, .. } => {
                assert_eq!(*align, vec![CellAlign::Center, CellAlign::Right]);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    fn table_cells(doc: &Document) -> (Vec<Vec<Span>>, Vec<Vec<Vec<Span>>>) {
        doc.blocks
            .iter()
            .find_map(|db| match &db.block {
                Block::Table { header, rows, .. } => Some((header.clone(), rows.clone())),
                _ => None,
            })
            .expect("table found")
    }

    #[test]
    fn table_and_tasklist() {
        let doc = parse("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n- [ ] todo\n");
        let (header, rows) = table_cells(&doc);
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 2);
    }

    #[test]
    fn tight_list_items() {
        let doc = parse("- one\n- two\n- three\n");
        let items = doc.blocks.iter().find_map(|db| match &db.block {
            Block::List { items, .. } => Some(items),
            _ => None,
        });
        let items = items.unwrap();
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0][0], Block::Paragraph(_)));
    }

    #[test]
    fn nested_list() {
        let doc = parse("- a\n  - b\n- c\n");
        let items = doc.blocks.iter().find_map(|db| match &db.block {
            Block::List { items, .. } => Some(items),
            _ => None,
        });
        let items = items.unwrap();
        assert_eq!(items.len(), 2);
        let nested = items[0]
            .iter()
            .any(|b| matches!(b, Block::List { items, .. } if items.len() == 1));
        assert!(nested);
    }

    #[test]
    fn nested_quote() {
        let doc = parse("> line\n> > deeper\n");
        match &doc.blocks[0].block {
            Block::Quote(depth, blocks) => {
                assert_eq!(*depth, 1);
                match &blocks[1] {
                    Block::Quote(2, inner) => assert_eq!(inner.len(), 1),
                    other => panic!("expected nested quote, got {other:?}"),
                }
            }
            other => panic!("expected quote, got {other:?}"),
        }
    }

    #[test]
    fn links_and_emphasis() {
        let doc = parse("a **b** [c](https://d.e)");
        match &doc.blocks[0].block {
            Block::Paragraph(spans) => {
                assert!(spans.iter().any(|s| s.bold && s.text == "b"));
                assert!(spans.iter().any(|s| s.link.as_deref() == Some("https://d.e")));
            }
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn headings_styles() {
        let doc = parse("## Title");
        match &doc.blocks[0].block {
            Block::Heading(2, spans) => {
                assert_eq!(spans[0].text, "Title".to_string());
                assert_eq!(spans[0].color, ColorKey::Heading);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn code_block_lang() {
        let doc = parse("```rust\nlet x = 1;\n```");
        match &doc.blocks[0].block {
            Block::CodeBlock { lang, text } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert!(text.contains("let x"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn ordered_list_start() {
        let doc = parse("# h\n\n3. three\n4. four\n");
        match &doc.blocks[1].block {
            Block::List { ordered, start, items } => {
                assert!(*ordered);
                assert_eq!(*start, 3);
                assert_eq!(items.len(), 2);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }
}
