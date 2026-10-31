//! Markdown rendering for egui.
//!
//! - every paragraph-like block is one wrapping `LayoutJob`/galley
//!   (real bold/italic cuts, line-height control, inline-code backgrounds)
//! - links get hit-boxes derived from glyph runs
//! - color emoji are reserved as blank sections and painted as images
//! - tables are laid out manually: natural column widths, borders, and a
//!   horizontal scroll when they are wider than the window
//! - code blocks are full width, syntax highlighted (syntect), one galley

use eframe::egui;
use std::rc::Rc;
use std::sync::Arc;

use margdawn_core::md::{Block, CellAlign, Span};
use margdawn_core::syn;
use margdawn_core::theme::{Color, ColorKey, Theme};

use crate::c32;
use crate::emoji::{self, EmojiStore, Piece};
use crate::fonts::{self, Face};
use crate::images::ImageStore;

/// Rendering knobs (sourced from config + active theme).
#[derive(Debug, Clone)]
pub struct RenderCfg {
    pub font_size: f32,
    pub line_height: f32,
    pub paragraph_spacing: f32,
    pub preserve_blank_lines: bool,
    pub code_theme: String,
    pub body: Face,
    pub heading: Face,
    /// forced max image width, as a percentage of the content width
    pub image_max_width_percent: Option<f32>,
}

impl RenderCfg {
    pub fn from_config(cfg: &margdawn_core::config::Config, theme: &Theme) -> Self {
        let body = Face::parse(cfg.font_body_override())
            .or_else(|| Face::parse(theme.font_body.as_deref()))
            .unwrap_or(Face::Sans);
        let heading = Face::parse(cfg.font_heading_override())
            .or_else(|| Face::parse(theme.font_heading.as_deref()))
            .unwrap_or(body);
        Self {
            font_size: cfg.font_size(),
            line_height: cfg.line_height(),
            paragraph_spacing: cfg.paragraph_spacing(),
            preserve_blank_lines: cfg.preserve_blank_lines(),
            code_theme: cfg.code_theme_name().to_string(),
            body,
            heading,
            image_max_width_percent: cfg.image_max_width_percent(),
        }
    }
}

/// Per-section data kept next to a `LayoutJob` (index == section index).
#[derive(Clone, Default)]
struct SectionMeta {
    link: Option<String>,
    emoji: Option<EmojiSlot>,
    /// inline code: background chip is painted manually (rounded + padded)
    inline_code: bool,
}

#[derive(Clone)]
struct EmojiSlot {
    key: String,
    /// total horizontal space reserved for the emoji (points)
    width: f32,
}

pub struct View {
    pub theme: Arc<Theme>,
    cfg: RenderCfg,
    emoji: Rc<EmojiStore>,
    images: Rc<ImageStore>,
    /// directory of the open document, for resolving relative image paths
    base_dir: Option<std::path::PathBuf>,
    widget_id: u64,
}

impl View {
    pub fn new(
        theme: Arc<Theme>,
        cfg: RenderCfg,
        emoji: Rc<EmojiStore>,
        images: Rc<ImageStore>,
        base_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self { theme, cfg, emoji, images, base_dir, widget_id: 0 }
    }

    // ---- document / spacing -------------------------------------------------

    pub fn render_document(&mut self, ui: &mut egui::Ui, doc: &margdawn_core::md::Document) {
        // all vertical rhythm is explicit (gap_between); egui adds nothing
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        let mut prev: Option<&Block> = None;
        for db in &doc.blocks {
            if let Some(p) = prev {
                let extra = if self.cfg.preserve_blank_lines { db.blanks_before } else { 0 };
                ui.add_space(
                    self.gap_between(p, &db.block)
                        + extra as f32 * self.cfg.paragraph_spacing,
                );
            }
            self.render_block(ui, &db.block);
            prev = Some(&db.block);
        }
    }

    /// Vertical gap between two sibling blocks.
    fn gap_between(&self, prev: &Block, next: &Block) -> f32 {
        let ps = self.cfg.paragraph_spacing;
        let f = self.cfg.font_size;
        match (prev, next) {
            (_, Block::Heading(level, _)) => {
                ps + f * match level {
                    1 => 1.0,
                    2 => 0.7,
                    3 => 0.45,
                    _ => 0.3,
                }
            }
            (Block::Heading(..), _) => ps * 0.6,
            _ => ps,
        }
    }

    fn render_children(&mut self, ui: &mut egui::Ui, blocks: &[Block]) {
        for (i, b) in blocks.iter().enumerate() {
            if i > 0 {
                ui.add_space(self.gap_between(&blocks[i - 1], b) * 0.6);
            }
            self.render_block(ui, b);
        }
    }

    fn next_id(&mut self) -> egui::Id {
        self.widget_id += 1;
        egui::Id::new(("margdawn-widget", self.widget_id))
    }

    fn heading_size(&self, level: u8) -> f32 {
        let f = self.cfg.font_size;
        match level {
            1 => f * 2.0,
            2 => f * 1.6,
            3 => f * 1.3,
            4 => f * 1.15,
            5 => f * 1.05,
            _ => f,
        }
    }

    fn neutral_line(&self) -> Color {
        self.theme
            .colors
            .table_border
            .unwrap_or_else(|| {
                self.theme
                    .colors
                    .foreground()
                    .blend(self.theme.colors.background(), 0.7)
            })
    }

    // ---- inline text -------------------------------------------------------

    /// Build the LayoutJob for a run of spans.
    fn build_job(
        &self,
        ctx: &egui::Context,
        spans: &[Span],
        base: ColorKey,
        size: f32,
        face: Face,
        bold_all: bool,
    ) -> (egui::text::LayoutJob, Vec<SectionMeta>) {
        let mut job = egui::text::LayoutJob::default();
        let mut meta: Vec<SectionMeta> = Vec::new();
        let colors = &self.theme.colors;
        let line_height = Some(size * self.cfg.line_height);
        let chip_pad_x = size * 0.24;
        let strike_col = c32(colors.resolve(ColorKey::Strike, ColorKey::Muted));
        // horizontal space still owed to the previous inline-code chip
        let mut leading_next = 0.0_f32;

        for span in spans {
            let mut color = colors.resolve(span.color, base);
            if span.color == ColorKey::Foreground && base == ColorKey::Foreground {
                if span.bold {
                    color = colors.strong.unwrap_or(color);
                } else if span.italic {
                    color = colors.emphasis.unwrap_or(color);
                }
            }
            let color = c32(color);
            let family = if span.code {
                egui::FontFamily::Monospace
            } else {
                fonts::family(face, span.bold || bold_all, span.italic)
            };
            let font_size = if span.code { size * 0.92 } else { size };
            let font_id = egui::FontId::new(font_size, family);
            // inline-code chips get padding before/after their text
            if span.code {
                job.append("", chip_pad_x, egui::TextFormat::default());
                meta.push(SectionMeta { link: None, emoji: None, inline_code: false });
                leading_next = 0.0;
            }
            let format = egui::TextFormat {
                font_id: font_id.clone(),
                line_height,
                color,
                background: egui::Color32::TRANSPARENT,
                underline: if span.link.is_some() {
                    egui::Stroke::new(1.0_f32, color)
                } else {
                    egui::Stroke::NONE
                },
                strikethrough: if span.strike {
                    egui::Stroke::new(1.0_f32, strike_col)
                } else {
                    egui::Stroke::NONE
                },
                valign: egui::Align::Center,
                ..Default::default()
            };

            for piece in emoji::split(&span.text) {
                match piece {
                    Piece::Text(t) => {
                        let lead = leading_next;
                        leading_next = 0.0;
                        job.append(t, lead, format.clone());
                        meta.push(SectionMeta {
                            link: span.link.clone(),
                            emoji: None,
                            inline_code: span.code,
                        });
                        if span.code {
                            leading_next = chip_pad_x;
                        }
                    }
                    Piece::Emoji(cluster) => match self.emoji.resolve(cluster) {
                        Some(key) => {
                            // blank, transparent placeholder wide enough for the image
                            let slot_font = egui::FontId::new(size, fonts::family(face, false, false));
                            let em = ctx.fonts(|f| f.glyph_width(&slot_font, '\u{2003}'));
                            let wanted = size * 1.3;
                            let leading = (wanted - em).max(0.0);
                            let blank = egui::TextFormat {
                                font_id: slot_font,
                                line_height,
                                color: egui::Color32::TRANSPARENT,
                                valign: egui::Align::Center,
                                ..Default::default()
                            };
                            let lead = leading_next;
                            leading_next = 0.0;
                            job.append("\u{2003}", lead + leading, blank);
                            meta.push(SectionMeta {
                                link: span.link.clone(),
                                emoji: Some(EmojiSlot {
                                    key,
                                    width: lead + leading + em,
                                }),
                                inline_code: false,
                            });
                        }
                        None => {
                            let lead = leading_next;
                            leading_next = 0.0;
                            job.append(cluster, lead, format.clone());
                            meta.push(SectionMeta {
                                link: span.link.clone(),
                                emoji: None,
                                inline_code: span.code,
                            });
                            if span.code {
                                leading_next = chip_pad_x;
                            }
                        }
                    },
                }
            }
        }
        (job, meta)
    }

    /// Paint a laid-out galley plus its color emoji.
    fn paint_galley(
        &self,
        ui: &egui::Ui,
        origin: egui::Pos2,
        galley: &Arc<egui::Galley>,
        meta: &[SectionMeta],
        size: f32,
    ) {
        // rounded, padded chips behind inline code, hugging the mono font box
        let chip_fill = c32(self.theme.colors.code_background());
        let chip_radius = (size * 0.26).max(2.0);
        let pad_x = size * 0.24;
        let pad_v = (size * 0.07).max(0.8);
        let descent = size * 0.92 * 0.30;
        for run in inline_code_runs(galley, meta) {
            let top = run.baseline - run.ascent + pad_v * 0.7;
            let bottom = run.baseline + descent + pad_v;
            let chip = egui::Rect::from_min_max(
                egui::pos2(origin.x + run.x0 - pad_x, origin.y + top),
                egui::pos2(origin.x + run.x1 + pad_x, origin.y + bottom),
            );
            ui.painter().rect_filled(chip, chip_radius, chip_fill);
        }
        ui.painter()
            .galley(origin, galley.clone(), c32(self.theme.colors.foreground()));
        for row in &galley.rows {
            for g in &row.glyphs {
                let Some(slot) = meta
                    .get(g.section_index as usize)
                    .and_then(|m| m.emoji.as_ref())
                else {
                    continue;
                };
                let Some(tex) = self.emoji.texture(ui.ctx(), &slot.key) else { continue };
                let tex_size = tex.size_vec2();
                let side = size * 1.12;
                let right = origin.x + g.pos.x + g.size.x;
                let center = egui::pos2(
                    right - slot.width / 2.0,
                    origin.y + row.rect.center().y,
                );
                let rect = egui::Rect::from_center_size(
                    center,
                    egui::vec2(side * tex_size.x / tex_size.y, side),
                );
                ui.painter().image(
                    tex.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
    }

    /// Hover cursor + click handling for links inside a painted galley.
    fn handle_links(
        &self,
        ui: &egui::Ui,
        origin: egui::Pos2,
        galley: &egui::text::Galley,
        meta: &[SectionMeta],
        resp: &egui::Response,
    ) {
        let rects = glyph_link_rects(galley, meta);
        if rects.is_empty() {
            return;
        }
        let pointer = ui.ctx().pointer_latest_pos();
        let over = |pos: egui::Pos2| {
            rects
                .iter()
                .find(|(r, _)| r.translate(origin.to_vec2()).contains(pos))
                .map(|(_, url)| url.clone())
        };
        if let Some(pos) = pointer {
            if resp.hovered() && over(pos).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }
        if resp.clicked() {
            if let Some(url) = pointer.and_then(over) {
                if url.contains("://") || url.starts_with("mailto:") {
                    ui.ctx().open_url(egui::OpenUrl::same_tab(url));
                }
            }
        }
    }

    /// A wrapped inline flow (paragraph, heading, list-item text).
    fn paint_inline(
        &mut self,
        ui: &mut egui::Ui,
        spans: &[Span],
        base: ColorKey,
        size: f32,
        face: Face,
        bold_all: bool,
    ) {
        if spans.is_empty() {
            return;
        }
        let (mut job, meta) = self.build_job(ui.ctx(), spans, base, size, face, bold_all);
        job.wrap.max_width = ui.available_width();
        let galley = ui.painter().layout_job(job);
        let (rect, resp) = ui.allocate_exact_size(galley.rect.size(), egui::Sense::click());
        self.paint_galley(ui, rect.min, &galley, &meta, size);
        self.handle_links(ui, rect.min, &galley, &meta, &resp);
    }

    // ---- blocks ------------------------------------------------------------

    fn render_block(&mut self, ui: &mut egui::Ui, block: &Block) {
        let ps = self.cfg.paragraph_spacing;
        match block {
            Block::Paragraph(spans) => {
                self.paint_inline(ui, spans, ColorKey::Foreground, self.cfg.font_size, self.cfg.body, false);
            }
            Block::Heading(level, spans) => {
                let heading = self.cfg.heading;
                self.paint_inline(ui, spans, ColorKey::Heading, self.heading_size(*level), heading, true);
            }
            Block::CodeBlock { lang, text } => self.render_code_block(ui, lang.as_deref(), text),
            Block::Quote(_depth, blocks) => {
                let bar = self
                    .theme
                    .colors
                    .quote_bar
                    .unwrap_or_else(|| self.theme.colors.foreground());
                let tint = self.theme.colors.background().blend(bar, 0.07);
                let resp = egui::Frame::none()
                    .fill(c32(tint))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.render_children(ui, blocks);
                    });
                let rect = resp.response.rect;
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        rect.left_top() + egui::vec2(0.0, 3.0),
                        egui::vec2(3.0, (rect.height() - 6.0).max(10.0)),
                    ),
                    2.0,
                    c32(bar),
                );
            }
            Block::List { ordered, start, items } => self.render_list(ui, *ordered, *start, items),
            Block::Table { align, header, rows } => self.render_table(ui, align, header, rows),
            Block::Image(image) => self.render_image(ui, image),
            Block::Rule => {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 2.0),
                    egui::Sense::hover(),
                );
                let hr = self.theme.colors.hr.unwrap_or_else(|| self.neutral_line());
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left(), rect.center().y),
                        egui::pos2(rect.right(), rect.center().y),
                    ],
                    egui::Stroke::new(1.5_f32, c32(hr)),
                );
            }
            Block::FootnoteDef { label, blocks } => {
                let muted = c32(self.theme.colors.resolve(ColorKey::Muted, ColorKey::Foreground));
                ui.horizontal_top(|ui| {
                    ui.label(
                        egui::RichText::new(format!("[{label}] "))
                            .color(muted)
                            .size(self.cfg.font_size * 0.85),
                    );
                    ui.vertical(|ui| self.render_children(ui, blocks));
                });
            }
        }
        let _ = ps;
    }

    fn render_image(&mut self, ui: &mut egui::Ui, image: &margdawn_core::md::ImageRef) {
        let alt = if image.alt.is_empty() { &image.src } else { &image.alt };
        match self.images.texture(ui.ctx(), self.base_dir.as_deref(), &image.src) {
            Some((texture, pixel_size)) => {
                let ppp = ui.ctx().pixels_per_point();
                let natural = pixel_size / ppp;
                let avail = ui.available_width();
                let mut width = natural.x.min(avail);
                if let Some(pct) = self.cfg.image_max_width_percent {
                    width = width.min(avail * pct / 100.0);
                }
                let scale = if natural.x > 0.0 { width / natural.x } else { 1.0 };
                let size = egui::vec2(width, (natural.y * scale).max(1.0));
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(avail, size.y), egui::Sense::hover());
                let image_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + (avail - size.x) / 2.0, rect.top()),
                    size,
                );
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                ui.add_space(2.0);
            }
            None => {
                let muted = c32(self.theme.colors.resolve(ColorKey::Muted, ColorKey::Foreground));
                let remote = image.src.starts_with("http://")
                    || image.src.starts_with("https://")
                    || image.src.starts_with("data:");
                let note = if remote {
                    "remote images are not supported"
                } else {
                    "image not found"
                };
                ui.label(
                    egui::RichText::new(format!("[image: {alt}] ({note})"))
                        .color(muted)
                        .italics(),
                );
            }
        }
    }

    fn render_code_block(&mut self, ui: &mut egui::Ui, lang: Option<&str>, text: &str) {
        let hl = syn::highlight(lang, text, &self.cfg.code_theme);
        let id = self.next_id();
        let size = self.cfg.font_size * 0.92;
        let line_height = Some(size * self.cfg.line_height.min(1.55));
        let font_id = fonts::mono_font_id(size);

        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        let default_fmt = egui::TextFormat {
            font_id: font_id.clone(),
            line_height,
            color: c32(hl.foreground),
            ..Default::default()
        };
        for (i, runs) in hl.lines.iter().enumerate() {
            if i > 0 {
                job.append("\n", 0.0, default_fmt.clone());
            }
            for (color, run) in runs {
                job.append(
                    run,
                    0.0,
                    egui::TextFormat { color: c32(*color), ..default_fmt.clone() },
                );
            }
        }
        if job.is_empty() {
            job.append(" ", 0.0, default_fmt.clone());
        }
        let galley = ui.painter().layout_job(job);

        egui::Frame::none()
            .fill(c32(hl.background))
            .rounding(6.0)
            .inner_margin(egui::Margin::symmetric(16.0, 12.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let avail = ui.available_width();
                if galley.rect.width() <= avail {
                    let (rect, _) =
                        ui.allocate_exact_size(galley.rect.size(), egui::Sense::hover());
                    ui.painter().galley(rect.min, galley.clone(), c32(hl.foreground));
                } else {
                    ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
                    egui::ScrollArea::horizontal()
                        .id_source(id)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(galley.rect.size(), egui::Sense::hover());
                            ui.painter().galley(rect.min, galley.clone(), c32(hl.foreground));
                        });
                }
            });
    }

    fn render_list(&mut self, ui: &mut egui::Ui, ordered: bool, start: u64, items: &[Vec<Block>]) {
        let marker_col = c32(
            self.theme
                .colors
                .list_marker
                .unwrap_or_else(|| self.theme.colors.foreground()),
        );
        let size = self.cfg.font_size;
        let slot_w = if ordered { size * 2.2 } else { size * 1.3 };
        let row_h = size * self.cfg.line_height;
        let body = self.cfg.body;

        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                ui.add_space(self.cfg.paragraph_spacing * 0.35);
            }
            let marker = if ordered {
                format!("{}.", start + i as u64)
            } else {
                "\u{2022}".to_string()
            };
            // horizontal_top: the marker stays aligned with the item's first line
            ui.horizontal_top(|ui| {
                let (slot, _) = ui.allocate_exact_size(egui::vec2(slot_w, row_h), egui::Sense::hover());
                ui.painter().text(
                    egui::pos2(slot.right() - size * 0.45, slot.center().y),
                    egui::Align2::RIGHT_CENTER,
                    marker,
                    egui::FontId::new(size, fonts::family(body, ordered, false)),
                    marker_col,
                );
                ui.vertical(|ui| self.render_children(ui, item));
            });
        }
    }

    fn render_table(
        &mut self,
        ui: &mut egui::Ui,
        align: &[CellAlign],
        header: &[Vec<Span>],
        rows: &[Vec<Vec<Span>>],
    ) {
        struct Cell {
            galley: Arc<egui::Galley>,
            meta: Vec<SectionMeta>,
        }

        let id = self.next_id();
        let size = self.cfg.font_size;
        let pad = egui::vec2(size * 0.8, size * 0.5);

        // 1. lay out every cell at its natural (unwrapped) size
        let mut grid: Vec<Vec<Cell>> = Vec::new();
        let mut header_rows = 0usize;
        let all_rows = std::iter::once(header)
            .filter(|h| !h.is_empty())
            .map(|h| (true, h))
            .chain(rows.iter().map(|r| (false, r.as_slice())));
        for (is_header, row) in all_rows {
            if is_header {
                header_rows += 1;
            }
            let mut cells = Vec::new();
            for spans in row {
                let (mut job, meta) = self.build_job(
                    ui.ctx(),
                    spans,
                    ColorKey::Foreground,
                    size,
                    self.cfg.body,
                    is_header,
                );
                job.wrap.max_width = f32::INFINITY;
                cells.push(Cell { galley: ui.painter().layout_job(job), meta });
            }
            grid.push(cells);
        }
        if grid.is_empty() {
            return;
        }

        // 2. column widths / row heights from content
        let cols = grid.iter().map(Vec::len).max().unwrap_or(0);
        let mut col_w = vec![size * 2.0; cols];
        let mut row_h = vec![size * self.cfg.line_height + pad.y * 2.0; grid.len()];
        for (r, row) in grid.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                col_w[c] = col_w[c].max(cell.galley.rect.width() + pad.x * 2.0);
                row_h[r] = row_h[r].max(cell.galley.rect.height() + pad.y * 2.0);
            }
        }
        let total = egui::vec2(col_w.iter().sum(), row_h.iter().sum());

        let border = egui::Stroke::new(1.0_f32, c32(self.neutral_line()));
        let head_fill = c32(
            self.theme
                .colors
                .background()
                .blend(self.theme.colors.foreground(), 0.07),
        );

        // 3. paint inside a horizontal scroll area (scrolls only when wider than the view)
        ui.scope(|ui| {
            ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
            egui::ScrollArea::horizontal()
                .id_source(id)
                .auto_shrink([true, true])
                .show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(total, egui::Sense::hover());
                    let painter = ui.painter().clone();

                    // header background
                    if header_rows > 0 {
                        let h: f32 = row_h.iter().take(header_rows).sum();
                        painter.rect_filled(
                            egui::Rect::from_min_size(rect.min, egui::vec2(total.x, h)),
                            0.0,
                            head_fill,
                        );
                    }

                    let mut y = rect.top();
                    for (r, row) in grid.iter().enumerate() {
                        let mut x = rect.left();
                        for (c, cell) in row.iter().enumerate() {
                            let a = align.get(c).copied().unwrap_or(CellAlign::Left);
                            let gw = cell.galley.rect.width();
                            let gh = cell.galley.rect.height();
                            let dx = match a {
                                CellAlign::Left => pad.x,
                                CellAlign::Center => (col_w[c] - gw) / 2.0,
                                CellAlign::Right => col_w[c] - pad.x - gw,
                            };
                            let origin = egui::pos2(x + dx, y + (row_h[r] - gh) / 2.0);
                            let cell_rect = egui::Rect::from_min_size(
                                egui::pos2(x, y),
                                egui::vec2(col_w[c], row_h[r]),
                            );
                            let resp = ui.interact(cell_rect, id.with((r, c)), egui::Sense::click());
                            self.paint_galley(ui, origin, &cell.galley, &cell.meta, size);
                            self.handle_links(ui, origin, &cell.galley, &cell.meta, &resp);
                            x += col_w[c];
                        }
                        y += row_h[r];
                    }

                    // borders: outer box + every column and row boundary
                    painter.rect_stroke(rect, 0.0, border);
                    let mut x = rect.left();
                    for w in col_w.iter().take(cols.saturating_sub(1)) {
                        x += w;
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            border,
                        );
                    }
                    let mut y = rect.top();
                    for h in row_h.iter().take(grid.len().saturating_sub(1)) {
                        y += h;
                        painter.line_segment(
                            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                            border,
                        );
                    }
                });
        });
    }
}

/// Rects (galley space) of every link section; per row, glyph runs merged.
fn glyph_link_rects(galley: &egui::text::Galley, meta: &[SectionMeta]) -> Vec<(egui::Rect, String)> {
    let mut out = Vec::new();
    for row in &galley.rows {
        let mut run: Option<(usize, f32, f32)> = None; // (section, x0, x1)
        for g in &row.glyphs {
            let sec = g.section_index as usize;
            let x_end = g.pos.x + g.size.x;
            match &mut run {
                Some((s, _, x2)) if *s == sec => *x2 = x_end,
                other => {
                    if let Some((s, x1, x2)) = other.take() {
                        if let Some(url) = meta.get(s).and_then(|m| m.link.clone()) {
                            out.push((run_rect(&row.rect, x1, x2), url));
                        }
                    }
                    run = Some((sec, g.pos.x, x_end));
                }
            }
        }
        if let Some((s, x1, x2)) = run.take() {
            if let Some(url) = meta.get(s).and_then(|m| m.link.clone()) {
                out.push((run_rect(&row.rect, x1, x2), url));
            }
        }
    }
    out
}

/// Geometry of one inline-code run on one row (galley space).
struct ChipRun {
    x0: f32,
    x1: f32,
    /// baseline of the run's first glyph
    baseline: f32,
    /// font ascent of the run's first glyph
    ascent: f32,
}

/// Collect inline-code runs, merged per row.
fn inline_code_runs(galley: &egui::text::Galley, meta: &[SectionMeta]) -> Vec<ChipRun> {
    let mut out = Vec::new();
    for row in &galley.rows {
        // (section, x0, x1, baseline, ascent)
        let mut run: Option<(usize, f32, f32, f32, f32)> = None;
        for g in &row.glyphs {
            let sec = g.section_index as usize;
            let is_code = meta.get(sec).is_some_and(|m| m.inline_code);
            let x_end = g.pos.x + g.size.x;
            match &mut run {
                Some((s, _, x2, _, _)) if *s == sec && is_code => *x2 = x_end,
                other => {
                    if let Some((s, x1, x2, baseline, ascent)) = other.take() {
                        if meta.get(s).is_some_and(|m| m.inline_code) {
                            out.push(ChipRun { x0: x1, x1: x2, baseline, ascent });
                        }
                    }
                    if is_code {
                        run = Some((sec, g.pos.x, x_end, g.pos.y, g.ascent));
                    }
                }
            }
        }
        if let Some((s, x1, x2, baseline, ascent)) = run.take() {
            if meta.get(s).is_some_and(|m| m.inline_code) {
                out.push(ChipRun { x0: x1, x1: x2, baseline, ascent });
            }
        }
    }
    out
}

fn run_rect(row: &egui::Rect, x1: f32, x2: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(x1.min(x2), row.top()),
        egui::pos2(x1.max(x2), row.bottom()),
    )
}
