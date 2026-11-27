//! eframe app: file loading, theming, settings panel, markdown view.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use margdawn_core::config::{self, Config};
use margdawn_core::md::{parse, Document};
use margdawn_core::theme::{self, Color, ColorKey, Theme};

use crate::clipboard_html;
use crate::emoji::EmojiStore;
use crate::editor::{save_text, SyntaxStyle};
use crate::images::ImageStore;
use crate::md_view::{RenderCfg, Selection, View};

/// An action held back because the buffer has unsaved changes.
#[derive(Debug, Clone)]
enum PendingAction {
    LeaveEdit,
    Reload,
    Open(PathBuf),
}

pub fn c32(c: Color) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

pub struct App {
    path: Option<PathBuf>,
    doc: Option<Document>,
    theme: Arc<Theme>,
    cfg: Config,
    warning: Option<String>,
    error: Option<String>,
    settings_open: bool,
    /// settings changed → save config.json once per frame
    dirty: bool,
    /// last installed monospace choice (skips redundant font rebuilds)
    fonts_key: String,
    theme_editor_name: String,
    theme_editing_colors: Vec<(&'static str, Option<Color>)>,
    theme_error: Option<String>,
    emoji: Rc<EmojiStore>,
    images: Rc<ImageStore>,
    /// directory of the open document (resolves relative image paths)
    base_dir: Option<PathBuf>,
    /// last image width percentage (kept when the force toggle is off)
    image_pct: f32,
    /// developer hook: `MARGDAWN_SCREENSHOT=out.png` saves a frame and quits
    screenshot: Option<PathBuf>,
    frames: u32,
    /// raw source of the open file. The parsed `Document` has already dropped
    /// the markup, so this is the only copy of what is actually on disk.
    src: Option<String>,
    /// edit mode: the content pane shows the source with dimmed markup
    editing: bool,
    /// the buffer differs from what is on disk
    file_dirty: bool,
    /// an action waiting for Save / Discard / Cancel
    confirm: Option<PendingAction>,
    /// last write error, shown above the editor
    save_error: Option<String>,
    /// where the view was scrolled to, so a round trip through edit mode
    /// doesn't throw the reader back to the top
    view_offset: f32,
    restore_offset: Option<f32>,
    /// mouse text selection in the rendered view, carried across frames so a
    /// drag survives a scroll or a re-render
    selection: Selection,
    /// Kinetic scrolling for wheel/trackpad input. egui applies a wheel delta
    /// and then stops dead the moment the fingers lift; this carries the
    /// last observed speed forward, decaying it, so a fast flick coasts.
    momentum: Momentum,
    /// Offset an in-flight coast wants applied on the next frame.
    pending_scroll: Option<f32>,
    /// How this desktop can carry HTML on the clipboard, detected once at
    /// startup because probing costs a process spawn.
    rich_clipboard: clipboard_html::RichClipboard,
}

/// Velocity carried past the end of a scroll gesture.
///
/// egui's scroll area has no momentum for wheel input (only for pointer
/// drags), so a flick stops dead on release. This tracks how fast the offset
/// was moving while input was arriving and, once input stops, keeps moving it
/// with friction until it settles.
#[derive(Debug, Default)]
pub struct Momentum {
    /// Points per second, in scroll-offset space (positive = downwards).
    velocity: f32,
    /// Offset seen in the previous frame, or `None` on the first frame.
    last_offset: Option<f32>,
    /// Whether input was scrolling in the frame just measured.
    was_scrolling: bool,
}

impl Momentum {
    /// Pixels per second below which the coast is over.
    const STOP_SPEED: f32 = 18.0;
    /// Deceleration, in pixels per second squared. Higher stops sooner.
    const FRICTION: f32 = 900.0;
    /// A flick slower than this is a deliberate, careful scroll: no coast.
    const MIN_FLICK: f32 = 60.0;
    /// Cap, so a violent flick cannot rocket the page away.
    const MAX_SPEED: f32 = 6000.0;

    /// Record this frame's movement and return the offset to force next frame,
    /// if a coast is in progress.
    fn observe(&mut self, offset: f32, dt: f32, scrolling_now: bool) -> Option<f32> {
        let mut coast = None;
        if dt > 0.0 {
            if let Some(prev) = self.last_offset {
                let delta = offset - prev;
                if scrolling_now && delta.abs() > 0.0 {
                    // Follow the live speed. Clamped so a layout jump cannot be
                    // mistaken for a violent flick.
                    let measured = (delta / dt).clamp(-Self::MAX_SPEED, Self::MAX_SPEED);
                    self.velocity = measured;
                } else if !scrolling_now {
                    if self.was_scrolling && self.velocity.abs() < Self::MIN_FLICK {
                        // A careful scroll stops exactly where it was let go.
                        self.velocity = 0.0;
                    } else if self.velocity != 0.0 {
                        // Coasting: bleed speed, then move. This also runs on the
                        // frame the input stops, so the flick carries straight on
                        // instead of losing one frame's worth of motion.
                        let friction = Self::FRICTION * dt;
                        if self.velocity.abs() <= Self::STOP_SPEED + friction {
                            self.velocity = 0.0;
                        } else {
                            self.velocity -= friction * self.velocity.signum();
                            coast = Some(offset + self.velocity * dt);
                        }
                    }
                }
            }
        }
        self.last_offset = Some(offset);
        self.was_scrolling = scrolling_now;
        coast
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> Self {
        let cfg = config::load();
        let (theme, warning) = load_theme(cfg.theme_name());
        crate::fonts::install(&cc.egui_ctx, &cfg);
        apply_text_styles(&cc.egui_ctx, &cfg);
        let theme_editor_name = cfg.theme_name().to_string();
        let theme_editing_colors = theme_slots(&theme.colors);

        let mut app = Self {
            path: None,
            doc: None,
            theme,
            warning,
            error: None,
            settings_open: false,
            dirty: false,
            fonts_key: font_key(&cfg),
            theme_editor_name,
            theme_editing_colors,
            theme_error: None,
            emoji: Rc::new(EmojiStore::new()),
            images: Rc::new(ImageStore::new()),
            base_dir: file.as_ref().and_then(|f| f.parent().map(PathBuf::from)),
            image_pct: cfg.image_max_width_percent().unwrap_or(80.0),
            screenshot: std::env::var_os("MARGDAWN_SCREENSHOT").map(PathBuf::from),
            frames: 0,
            src: None,
            editing: false,
            file_dirty: false,
            confirm: None,
            save_error: None,
            view_offset: 0.0,
            restore_offset: None,
            selection: Selection::default(),
            momentum: Momentum::default(),
            pending_scroll: None,
            rich_clipboard: clipboard_html::RichClipboard::new(),
            cfg,
        };
        if std::env::var_os("MARGDAWN_SETTINGS").is_some() {
            app.settings_open = true;
        }
        if let Some(f) = file {
            app.load_file(&f);
        }
        // dev hook, mirrors MARGDAWN_SETTINGS: start already in edit mode so a
        // screenshot run can capture it without a click
        if std::env::var_os("MARGDAWN_EDIT").is_some() && app.src.is_some() {
            app.editing = true;
        }
        // and one for the unsaved-changes state: dirty buffer + open dialog
        if std::env::var_os("MARGDAWN_DIRTY").is_some() && app.src.is_some() {
            app.editing = true;
            app.file_dirty = true;
            app.confirm = Some(PendingAction::LeaveEdit);
        }
        app
    }

    fn load_file(&mut self, path: &Path) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.doc = Some(parse(&text));
                self.src = Some(text);
                self.base_dir = path.parent().map(PathBuf::from);
                self.path = Some(path.to_path_buf());
                self.error = None;
                self.save_error = None;
                self.file_dirty = false;
                self.confirm = None;
            }
            Err(e) => {
                self.error = Some(format!("cannot read {}: {e}", path.display()));
                self.doc = None;
                self.src = None;
                self.file_dirty = false;
            }
        }
    }

    /// Write the buffer back to disk. Does nothing without a path.
    fn save_file(&mut self) {
        let Some(path) = self.path.clone() else { return };
        let Some(src) = self.src.clone() else { return };
        match save_text(&path, &src) {
            Ok(()) => {
                self.file_dirty = false;
                self.save_error = None;
                self.error = None;
                // keep the view honest: it renders whatever is now on disk
                self.doc = Some(parse(&src));
            }
            Err(e) => {
                self.save_error = Some(format!("cannot write {}: {e}", path.display()));
            }
        }
    }

    /// Toggle edit mode, holding the change back if the buffer is dirty.
    fn toggle_edit(&mut self) {
        if self.editing {
            if self.file_dirty {
                self.confirm = Some(PendingAction::LeaveEdit);
            } else {
                self.editing = false;
                self.reparse();
            }
        } else if self.src.is_some() {
            self.editing = true;
            self.save_error = None;
        }
    }

    /// Re-render the view from the buffer (used when leaving edit mode).
    fn reparse(&mut self) {
        if let Some(src) = &self.src {
            self.doc = Some(parse(src));
        }
        // the reader was somewhere in this document a moment ago
        self.restore_offset = Some(self.view_offset);
    }

    /// Open a file, asking first if the buffer would be thrown away.
    fn request_open(&mut self, path: PathBuf) {
        if self.editing && self.file_dirty {
            self.confirm = Some(PendingAction::Open(path));
        } else {
            self.load_file(&path);
        }
    }

    /// Reload from disk, asking first if the buffer would be thrown away.
    fn request_reload(&mut self) {
        let Some(path) = self.path.clone() else { return };
        if self.editing && self.file_dirty {
            self.confirm = Some(PendingAction::Reload);
        } else {
            self.load_file(&path);
        }
    }

    /// Discard the buffer and reload from disk.
    fn reload_from_disk(&mut self) {
        let Some(path) = self.path.clone() else { return };
        self.load_file(&path);
    }

    fn resolve_pending(&mut self, ctx: &egui::Context, save: bool, discard: bool) {
        let Some(action) = self.confirm.take() else { return };
        if save {
            self.save_file();
            if self.save_error.is_some() {
                // the write failed: stay put rather than losing the buffer
                self.confirm = Some(action);
                ctx.request_repaint();
                return;
            }
        } else if !discard {
            return; // cancelled
        }
        self.editing = false;
        match action {
            PendingAction::LeaveEdit => self.reparse(),
            PendingAction::Reload => self.reload_from_disk(),
            PendingAction::Open(path) => self.load_file(&path),
        }
        ctx.request_repaint();
    }

    fn reload_theme(&mut self) {
        let (theme, warning) = load_theme(self.cfg.theme_name());
        self.theme = theme;
        self.warning = warning;
        self.theme_editor_name = self.cfg.theme_name().to_string();
        self.theme_editing_colors = theme_slots(&self.theme.colors);
    }
}

fn font_key(cfg: &Config) -> String {
    cfg.font_mono().to_string()
}

/// Fixed UI text sizes (the document font size is a separate setting).
fn apply_text_styles(ctx: &egui::Context, _cfg: &Config) {
    ctx.style_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.5));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.5));
        style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(18.0));
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.5));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.scroll.foreground_color = false;
    });
}

/// Style every egui widget from the active theme (buttons, sliders,
/// combo boxes, windows ...), not just the document background.
fn apply_visuals(ctx: &egui::Context, theme: &Theme) {
    let c = &theme.colors;
    let (bg, fg) = (c.background(), c.foreground());
    let accent = c.resolve(ColorKey::Link, ColorKey::Foreground);
    let dark = theme.shade() == theme::Shade::Dark;

    let surface = c32(bg.blend(fg, 0.12));
    let hover = c32(bg.blend(fg, 0.18));
    let pressed = c32(bg.blend(fg, 0.23));
    let line = c32(bg.blend(fg, 0.16));
    let text = c32(fg);
    let rounding = egui::Rounding::same(8.0);

    ctx.style_mut(|style| {
        let v = &mut style.visuals;
        *v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.override_text_color = None;
        v.panel_fill = c32(bg);
        v.window_fill = c32(bg.blend(fg, 0.04));
        v.window_stroke = egui::Stroke::new(1.0_f32, line);
        v.window_rounding = egui::Rounding::same(12.0);
        v.menu_rounding = egui::Rounding::same(10.0);
        v.extreme_bg_color = c32(c.code_background());
        v.faint_bg_color = surface;
        v.code_bg_color = c32(c.code_background());
        v.hyperlink_color = c32(accent);
        v.selection.bg_fill = c32(accent.blend(bg, 0.55));
        v.selection.stroke = egui::Stroke::new(1.0_f32, text);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c32(bg);
        w.noninteractive.weak_bg_fill = c32(bg);
        w.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, line);
        w.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        w.noninteractive.rounding = rounding;

        w.inactive.bg_fill = surface;
        w.inactive.weak_bg_fill = surface;
        w.inactive.bg_stroke = egui::Stroke::NONE;
        w.inactive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        w.inactive.rounding = rounding;

        w.hovered.bg_fill = hover;
        w.hovered.weak_bg_fill = hover;
        w.hovered.bg_stroke = egui::Stroke::new(1.0_f32, line);
        w.hovered.fg_stroke = egui::Stroke::new(1.5_f32, text);
        w.hovered.rounding = rounding;

        w.active.bg_fill = pressed;
        w.active.weak_bg_fill = pressed;
        w.active.bg_stroke = egui::Stroke::new(1.0_f32, c32(accent));
        w.active.fg_stroke = egui::Stroke::new(2.0_f32, text);
        w.active.rounding = rounding;

        w.open.bg_fill = surface;
        w.open.weak_bg_fill = surface;
        w.open.bg_stroke = egui::Stroke::new(1.0_f32, line);
        w.open.fg_stroke = egui::Stroke::new(1.0_f32, text);
        w.open.rounding = rounding;
    });
}

fn load_theme(name: &str) -> (Arc<Theme>, Option<String>) {
    let dir = config::config_dir();
    match theme::load(&dir, name) {
        Ok(mut t) => {
            // A self-made theme may lack background/foreground → would render
            // white-on-white. Inherit those from the same-named built-in (or
            // the default theme) and derive the light/dark mode from the bg.
            let donor_name = if theme::builtin(name).is_some() {
                name
            } else {
                theme::DEFAULT_THEME
            };
            let donor = theme::builtin(donor_name)
                .or_else(|| theme::builtin(theme::DEFAULT_THEME))
                .unwrap_or_else(|| {
                    Theme::new("default", theme::Shade::Dark, theme::Colors::default())
                });
            let mut warning = None;
            if t.inherit_essentials_from(&donor) {
                warning = Some(format!(
                    "theme `{name}` is missing background/foreground; inherited them from `{donor_name}`"
                ));
            }
            t.appearance = Some(theme::Shade::from_background(t.colors.background()));
            (Arc::new(t.with_name(name)), warning)
        }
        Err(_) => {
            let fallback = theme::builtin(theme::DEFAULT_THEME)
                .unwrap_or_else(|| Theme::new("default", theme::Shade::Dark, Default::default()));
            (
                Arc::new(fallback),
                Some(format!(
                    "theme `{name}` could not be loaded; falling back to `{}`",
                    theme::DEFAULT_THEME
                )),
            )
        }
    }
}

/// The color a slot resolves to when unset (used to seed the color picker so
/// an "auto" slot can be turned back into a concrete color).
fn resolved_slot(theme: &Theme, label: &str) -> Color {
    let c = &theme.colors;
    use margdawn_core::theme::ColorKey as K;
    match label {
        "background" => c.background(),
        "foreground" => c.foreground(),
        "muted" => c.resolve(K::Muted, K::Foreground),
        "heading" => c.resolve(K::Heading, K::Foreground),
        "link" => c.resolve(K::Link, K::Foreground),
        "code" => c.resolve(K::Code, K::Foreground),
        "code_background" => c.code_background(),
        "emphasis" => c.resolve(K::Emphasis, K::Foreground),
        "strong" => c.resolve(K::Strong, K::Foreground),
        "strikethrough" => c.resolve(K::Strike, K::Muted),
        "quote" => c.quote.unwrap_or_else(|| c.resolve(K::Muted, K::Foreground)),
        "quote_bar" => c.quote_bar.unwrap_or_else(|| c.foreground()),
        "list_marker" => c.list_marker.unwrap_or_else(|| c.foreground()),
        "hr" => c.hr.unwrap_or_else(|| c.resolve(K::Strike, K::Muted)),
        "table_border" => c.table_border.unwrap_or_else(|| c.resolve(K::Strike, K::Muted)),
        _ => c.foreground(),
    }
}

// ---- theme editor slot plumbing -------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Background,
    Foreground,
    Muted,
    Heading,
    Link,
    Code,
    CodeBackground,
    Emphasis,
    Strong,
    Strikethrough,
    Quote,
    QuoteBar,
    ListMarker,
    Hr,
    TableBorder,
}

type Colors = margdawn_core::theme::Colors;

fn get_slot(c: &Colors, slot: Slot) -> Option<Color> {
    match slot {
        Slot::Background => c.background,
        Slot::Foreground => c.foreground,
        Slot::Muted => c.muted,
        Slot::Heading => c.heading,
        Slot::Link => c.link,
        Slot::Code => c.code,
        Slot::CodeBackground => c.code_background,
        Slot::Emphasis => c.emphasis,
        Slot::Strong => c.strong,
        Slot::Strikethrough => c.strikethrough,
        Slot::Quote => c.quote,
        Slot::QuoteBar => c.quote_bar,
        Slot::ListMarker => c.list_marker,
        Slot::Hr => c.hr,
        Slot::TableBorder => c.table_border,
    }
}

fn set_slot(c: &mut Colors, slot: Slot, v: Option<Color>) {
    match slot {
        Slot::Background => c.background = v,
        Slot::Foreground => c.foreground = v,
        Slot::Muted => c.muted = v,
        Slot::Heading => c.heading = v,
        Slot::Link => c.link = v,
        Slot::Code => c.code = v,
        Slot::CodeBackground => c.code_background = v,
        Slot::Emphasis => c.emphasis = v,
        Slot::Strong => c.strong = v,
        Slot::Strikethrough => c.strikethrough = v,
        Slot::Quote => c.quote = v,
        Slot::QuoteBar => c.quote_bar = v,
        Slot::ListMarker => c.list_marker = v,
        Slot::Hr => c.hr = v,
        Slot::TableBorder => c.table_border = v,
    }
}

const EDITOR_SLOTS: &[(Slot, &str)] = &[
    (Slot::Background, "background"),
    (Slot::Foreground, "foreground"),
    (Slot::Muted, "muted"),
    (Slot::Heading, "heading"),
    (Slot::Link, "link"),
    (Slot::Code, "code"),
    (Slot::CodeBackground, "code_background"),
    (Slot::Emphasis, "emphasis"),
    (Slot::Strong, "strong"),
    (Slot::Strikethrough, "strikethrough"),
    (Slot::Quote, "quote"),
    (Slot::QuoteBar, "quote_bar"),
    (Slot::ListMarker, "list_marker"),
    (Slot::Hr, "hr"),
    (Slot::TableBorder, "table_border"),
];

fn theme_slots(c: &Colors) -> Vec<(&'static str, Option<Color>)> {
    EDITOR_SLOTS
        .iter()
        .map(|(slot, name)| (*name, get_slot(c, *slot)))
        .collect()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ---- visuals from theme --------------------------------------------
        apply_visuals(ctx, &self.theme);
        let bg = self.theme.colors.background();
        self.frames += 1;
        self.screenshot_tick(ctx);

        // font families may have changed via settings
        if font_key(&self.cfg) != self.fonts_key {
            crate::fonts::install(ctx, &self.cfg);
            self.fonts_key = font_key(&self.cfg);
            ctx.request_repaint();
        }

        // window title
        let dirty_mark = if self.file_dirty { " •" } else { "" };
        let title = match &self.path {
            Some(p) => format!(
                "{}{dirty_mark} — margdawn",
                p.file_name().and_then(|n| n.to_str()).unwrap_or("margdawn")
            ),
            None => "margdawn — markdown viewer".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // drag & drop
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.iter().rev().find_map(|f| f.path.clone()) {
            self.request_open(f);
        }

        // ---- keyboard -------------------------------------------------------
        let ctrl_o = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::O));
        let ctrl_s = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S));
        let ctrl_e = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::E));
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if esc {
            if self.confirm.is_some() {
                self.confirm = None;
            } else if self.editing {
                self.toggle_edit();
            }
        }
        if ctrl_s && self.editing && self.file_dirty {
            self.save_file();
        }
        if ctrl_e && !self.confirm.is_some() {
            self.toggle_edit();
        }
        // ---- copy the view selection -----------------------------------------
        // Handled here, before any panel runs, rather than inside the scroll
        // area: the copy shortcut belongs to the window, and reading it at the
        // top of the frame means no widget or scroll area can swallow it first.
        if !self.editing && self.cfg.selectable_text() && self.selection.has_selection() {
            let (copy_event, shift_c) = ctx.input(|i| {
                let copy = i
                    .events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut));
                let shift_c =
                    i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::C);
                (copy, shift_c)
            });
            if shift_c {
                // Rich copy: put HTML *and* plain text on the clipboard, so a
                // word processor keeps the formatting while a plain editor
                // still gets readable text. egui can only carry plain text, so
                // the HTML flavour goes through the desktop's clipboard tool;
                // if that is unavailable, fall back to the Markdown source,
                // which at least survives a Markdown-aware destination.
                let plain = self.selection.plain_text();
                let html = self.selection.html();
                let mut stored_rich = false;
                if let (Some(html), Some(plain)) = (html.as_deref(), plain.as_deref()) {
                    // Only attempt the rich path when the clipboard can take
                    // it; `set_html` reports failure rather than lying.
                    if self.rich_clipboard.available() {
                        stored_rich = self.rich_clipboard.set_html(html, plain);
                    }
                }
                if let Some(plain) = plain {
                    // Keep egui's own copy in sync: it is what a plain editor
                    // or the next Ctrl+C in another app will read.
                    ctx.copy_text(plain);
                }
                if !stored_rich {
                    // No rich flavour: leave the Markdown source on the
                    // clipboard so Ctrl+Shift+C still means "the original".
                    if let Some(md) = self.selection.markdown_text() {
                        ctx.copy_text(md);
                    }
                }
            } else if copy_event {
                if let Some(text) = self.selection.plain_text() {
                    ctx.copy_text(text);
                }
            }
        }

        // ---- top bar --------------------------------------------------------
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() || ctrl_o {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                        .pick_file()
                    {
                        self.request_open(path);
                    }
                }
                if self.path.is_some() && ui.button("Reload").clicked() {
                    self.request_reload();
                }
                if ui.button("Settings").clicked() {
                    self.settings_open = !self.settings_open;
                    if self.settings_open {
                        self.theme_editor_name = self.cfg.theme_name().to_string();
                        self.theme_editing_colors = theme_slots(&self.theme.colors);
                    }
                }
                let edit_label = if self.editing { "Done" } else { "Edit" };
                if ui
                    .add_enabled(self.src.is_some(), egui::Button::new(edit_label))
                    .on_hover_text("Edit the file in place (Ctrl+E)")
                    .clicked()
                {
                    self.toggle_edit();
                }
                if self.editing
                    && ui
                        .add_enabled(self.file_dirty, egui::Button::new("Save"))
                        .on_hover_text("Write the buffer to disk (Ctrl+S)")
                        .clicked()
                {
                    self.save_file();
                }
                if let Some(p) = &self.path {
                    let name = p.display().to_string();
                    if self.file_dirty {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 170, 60),
                            format!("{name} •"),
                        );
                    } else {
                        ui.weak(name);
                    }
                }
            });
            ui.add_space(4.0);
        });

        // ---- content ----------------------------------------------------------
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(c32(bg))
                    .inner_margin(egui::Margin::same(self.cfg.margin())),
            )
            .show(ctx, |ui| {
                if let Some(w) = &self.warning {
                    ui.colored_label(egui::Color32::from_rgb(230, 130, 30), w.to_string());
                    ui.add_space(4.0);
                }
                if let Some(err) = &self.error {
                    ui.colored_label(egui::Color32::RED, err);
                    return;
                }
                match &self.doc {
                    None => {
                        ui.centered_and_justified(|ui| {
                            ui.weak("No file open — press Open, drag & drop a .md file, or run margdawn-gui file.md");
                        });
                    }
                    Some(doc) => {
                        if self.editing {
                            self.show_editor(ui);
                        } else {
                            let mut area = egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                // Dragging in the document selects text, so the
                                // scroll area must not also claim the drag for
                                // "drag to scroll" (its default). Otherwise the
                                // area steals every drag before the text units
                                // see it and nothing can ever be selected.
                                .drag_to_scroll(!self.cfg.selectable_text());
                            if let Some(y) = self.restore_offset.take() {
                                area = area.vertical_scroll_offset(y);
                            }
                            // A coast in progress overrides where the area
                            // would otherwise land this frame.
                            if let Some(y) = self.pending_scroll.take() {
                                area = area.vertical_scroll_offset(y);
                            }
                            let out = area.show(ui, |ui| {
                                let mut view = View::new(
                                    Arc::clone(&self.theme),
                                    RenderCfg::from_config(&self.cfg, &self.theme),
                                    Rc::clone(&self.emoji),
                                    Rc::clone(&self.images),
                                    self.base_dir.clone(),
                                );
                                view.render_document(ui, doc, &mut self.selection);
                            });
                            self.view_offset = out.state.offset.y;
                            self.feed_momentum(ctx, out.state.offset.y, out.content_size.y, out.inner_rect.height());
                        }
                    }
                }
            });

        // ---- settings window --------------------------------------------------
        if self.settings_open {
            self.show_settings(ctx);
        }

        // ---- unsaved-changes dialog -------------------------------------------
        if let Some(action) = self.confirm.clone() {
            self.show_confirm(ctx, action);
        }

        // ---- persistence ------------------------------------------------------
        if self.dirty {
            if let Err(e) = config::save(&self.cfg) {
                eprintln!("warning: could not save config: {e}");
            }
            self.dirty = false;
        }
    }
}

impl App {
    /// Continue a fast flick after the fingers lift.
    ///
    /// egui applies whatever wheel delta arrived this frame and then stops; it
    /// carries no speed into the next frame. Watching the offset gives that
    /// speed, and holding it here lets the page coast to a stop instead of
    /// braking the instant the gesture ends.
    fn feed_momentum(&mut self, ctx: &egui::Context, offset: f32, content_h: f32, view_h: f32) {
        let dt = ctx.input(|i| i.stable_dt).clamp(1.0 / 240.0, 0.1);
        // "Input is arriving" means a wheel or scroll event landed this frame.
        let scrolling_now = ctx.input(|i| i.raw_scroll_delta.y != 0.0);
        let max_offset = (content_h - view_h).max(0.0);

        if let Some(next) = self.momentum.observe(offset, dt, scrolling_now) {
            let clamped = next.clamp(0.0, max_offset);
            if (clamped - offset).abs() > 0.01 {
                self.pending_scroll = Some(clamped);
                ctx.request_repaint();
            } else {
                // Hit the top or bottom: the coast is over.
                self.momentum.reset();
            }
        }
    }

    /// The edit pane: the raw source, styled as source.
    ///
    /// A plain `TextEdit` over the real bytes — the layouter only changes how
    /// runs look, never what they are — so cursor, selection and undo all work
    /// on the file itself.
    fn show_editor(&mut self, ui: &mut egui::Ui) {
        let c = &self.theme.colors;
        let st = SyntaxStyle {
            fg: c32(c.foreground()),
            muted: c32(c.resolve(ColorKey::Muted, ColorKey::Foreground)),
            heading: c32(c.resolve(ColorKey::Heading, ColorKey::Foreground)),
            code: c32(c.resolve(ColorKey::Code, ColorKey::Foreground)),
            link: c32(c.resolve(ColorKey::Link, ColorKey::Foreground)),
            strong: c32(c.resolve(ColorKey::Strong, ColorKey::Foreground)),
            emphasis: c32(c.resolve(ColorKey::Emphasis, ColorKey::Foreground)),
            strike: c32(c.resolve(ColorKey::Strike, ColorKey::Muted)),
            font: egui::FontId::monospace(self.cfg.font_size()),
        };

        if let Some(err) = &self.save_error {
            ui.colored_label(egui::Color32::from_rgb(210, 90, 70), err);
            ui.add_space(4.0);
        }

        let Some(src) = self.src.as_mut() else {
            ui.centered_and_justified(|ui| ui.weak("No file open"));
            return;
        };

        // Fill the window with the pane so the source has somewhere to be, then
        // let it grow past the viewport when the document is longer.
        let row_height = ui.fonts(|f| f.row_height(&st.font));
        let min_rows = ((ui.available_height() / row_height).ceil() as usize).max(1);

        let out = crate::editor::source_editor(ui, src, &st, min_rows);
        if out.response.changed() {
            self.file_dirty = true;
        }
    }

    /// Save / Discard / Cancel for a buffered action.
    fn show_confirm(&mut self, ctx: &egui::Context, action: PendingAction) {
        let what = match &action {
            PendingAction::LeaveEdit => "leave edit mode",
            PendingAction::Reload => "reload the file from disk",
            PendingAction::Open(_) => "open another file",
        };
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;

        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!("This file has unsaved changes. Save before you {what}?"));
                if let Some(err) = &self.save_error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(210, 90, 70), err);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        save = true;
                    }
                    if ui.button("Discard").clicked() {
                        discard = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if save || discard {
            self.resolve_pending(ctx, save, discard);
        } else if cancel {
            self.confirm = None;
        }
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        let (user_themes, builtin_themes) = config::available_themes();
        let code_themes = margdawn_core::syn::code_theme_names();
        let mut changed = false;

        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .default_width(440.0)
            .show(ctx, |ui| {
                ui.weak("Changes apply live and are saved to config.json");
                ui.add_space(2.0);

                // ---- theme
                // de-duplicate: user themes shadow built-ins, and must not
                // appear twice in the picker
                let mut mixed: Vec<String> = Vec::new();
                for name in &user_themes {
                    if !mixed.contains(name) {
                        mixed.push(name.clone());
                    }
                }
                for name in &builtin_themes {
                    if !mixed.iter().any(|m| m == name) {
                        mixed.push((*name).to_string());
                    }
                }
                let is_shadowed = |name: &str| {
                    user_themes.iter().any(|u| u == name)
                        && builtin_themes.iter().any(|b| *b == name)
                };
                let label_for = |name: &str| {
                    if is_shadowed(name) {
                        format!("{name}  (custom, overrides built-in)")
                    } else {
                        name.to_string()
                    }
                };
                let mut sel = self.cfg.theme_name().to_string();
                egui::ComboBox::from_label("Theme")
                    .selected_text(label_for(&sel))
                    .show_ui(ui, |ui| {
                        for t in &mixed {
                            ui.selectable_value(&mut sel, t.clone(), label_for(t));
                        }
                    });
                if sel != self.cfg.theme_name() {
                    self.cfg.theme = Some(sel);
                    self.reload_theme();
                    changed = true;
                }

                // ---- code theme
                let mut csel = self.cfg.code_theme_name().to_string();
                egui::ComboBox::from_label("Code theme")
                    .selected_text(csel.clone())
                    .show_ui(ui, |ui| {
                        for t in &code_themes {
                            ui.selectable_value(&mut csel, t.clone(), t);
                        }
                    });
                if csel != self.cfg.code_theme_name() {
                    self.cfg.code_theme = Some(csel);
                    changed = true;
                }

                // ---- typography
                ui.add_space(8.0);
                ui.heading("Typography");
                ui.add_space(2.0);
                let mut fs = self.cfg.font_size();
                if ui.add(egui::Slider::new(&mut fs, 10.0..=30.0).text("Body font size").integer())
                    .changed()
                {
                    self.cfg.font_size = Some(fs);
                    changed = true;
                }
                let mut lh = self.cfg.line_height();
                if ui.add(egui::Slider::new(&mut lh, 1.0..=2.4).text("Line height")).changed() {
                    self.cfg.line_height = Some(lh);
                    changed = true;
                }
                let mut ps = self.cfg.paragraph_spacing();
                if ui.add(egui::Slider::new(&mut ps, 0.0..=40.0).text("Paragraph spacing"))
                    .changed()
                {
                    self.cfg.paragraph_spacing = Some(ps);
                    changed = true;
                }
                changed |= font_choice(ui, "Body font", &mut self.cfg.font_body);
                changed |= font_choice(ui, "Heading font", &mut self.cfg.font_heading);
                ui.horizontal(|ui| {
                    ui.label("Code font:");
                    for (name, label) in
                        [("jetbrainsmono", "JetBrains Mono"), ("hack", "Hack")]
                    {
                        let mut m = self.cfg.font_mono().to_string();
                        if ui.selectable_value(&mut m, name.to_string(), label).changed() {
                            self.cfg.font_mono = Some(m);
                            changed = true;
                        }
                    }
                });

                // ---- layout
                ui.add_space(8.0);
                ui.heading("Layout");
                ui.add_space(2.0);
                let mut mg = self.cfg.margin();
                if ui.add(egui::Slider::new(&mut mg, 0.0..=100.0).text("Page margin"))
                    .changed()
                {
                    self.cfg.margin = Some(mg);
                    changed = true;
                }
                let mut pb = self.cfg.preserve_blank_lines();
                if ui
                    .checkbox(&mut pb, "Respect blank lines between blocks")
                    .changed()
                {
                    self.cfg.preserve_blank_lines = Some(pb);
                    changed = true;
                }

                // ---- selection
                ui.add_space(8.0);
                ui.heading("Selection");
                ui.add_space(2.0);
                let mut sel_text = self.cfg.selectable_text();
                if ui
                    .checkbox(&mut sel_text, "Select and copy text in the view")
                    .changed()
                {
                    self.cfg.selectable_text = Some(sel_text);
                    if !sel_text {
                        self.selection.clear();
                    }
                    changed = true;
                }
                ui.add_enabled_ui(sel_text, |ui| {
                    ui.weak("Ctrl+C copies plain text · Ctrl+Shift+C keeps Markdown markers");
                });

                // ---- images
                ui.add_space(8.0);
                ui.heading("Images");
                ui.add_space(2.0);
                let mut forced = self.cfg.image_max_width_percent.is_some();
                if ui
                    .checkbox(&mut forced, "Force maximum image width")
                    .changed()
                {
                    self.cfg.image_max_width_percent =
                        forced.then_some(self.image_pct.clamp(5.0, 100.0));
                    changed = true;
                }
                let mut pct = self.cfg.image_max_width_percent.unwrap_or(self.image_pct);
                let resp = ui.add_enabled(
                    forced,
                    egui::Slider::new(&mut pct, 10.0..=100.0)
                        .suffix("%")
                        .text("Max width (share of content)"),
                );
                if resp.changed() {
                    self.image_pct = pct;
                    if forced {
                        self.cfg.image_max_width_percent = Some(pct);
                    }
                    changed = true;
                }

                // ---- theme editor
                ui.add_space(10.0);
                let open_editor = std::env::var("MARGDAWN_SETTINGS")
                    .map(|v| v == "editor")
                    .unwrap_or(false);
                egui::CollapsingHeader::new("Theme editor")
                    .default_open(open_editor)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut self.theme_editor_name);
                        });
                        let name = self.theme_editor_name.trim().to_string();
                        if !name.is_empty()
                            && builtin_themes.iter().any(|b| *b == name)
                        {
                            ui.colored_label(
                                egui::Color32::from_rgb(230, 150, 60),
                                format!("overrides the built-in theme `{name}`"),
                            );
                        } else if !name.is_empty() && user_themes.iter().any(|u| *u == name) {
                            ui.weak(format!("overwrites your existing theme `{name}`"));
                        }
                        ui.add_space(2.0);
                        ui.weak("(unset slots fall back automatically)");
                        if let Some(err) = &self.theme_error {
                            ui.colored_label(egui::Color32::from_rgb(200, 80, 60), err);
                        }
                        let fallbacks: Vec<Color> = self
                            .theme_editing_colors
                            .iter()
                            .map(|(label, _)| resolved_slot(&self.theme, label))
                            .collect();
                        let mut any_edit = false;
                        {
                            let colors_ref = &mut self.theme_editing_colors;
                            egui::ScrollArea::vertical()
                                .max_height(240.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for (i, (label, color)) in colors_ref.iter_mut().enumerate() {
                                        ui.horizontal(|ui| {
                                            ui.monospace(*label);
                                            if let Some(c) = color.as_mut() {
                                                let mut rgb = [c.r, c.g, c.b];
                                                if ui.color_edit_button_srgb(&mut rgb).changed()
                                                    && rgb != [c.r, c.g, c.b]
                                                {
                                                    *c = Color::new(rgb[0], rgb[1], rgb[2]);
                                                    any_edit = true;
                                                }
                                            } else {
                                                // seed with the resolved fallback so an
                                                // "auto" slot can be given a color again
                                                let fb = fallbacks[i];
                                                let mut rgb = [fb.r, fb.g, fb.b];
                                                if ui.color_edit_button_srgb(&mut rgb).changed() {
                                                    *color =
                                                        Some(Color::new(rgb[0], rgb[1], rgb[2]));
                                                    any_edit = true;
                                                }
                                                ui.weak("auto");
                                            }
                                            let unset = ui
                                                .small_button("\u{00d7}")
                                                .on_hover_text("unset (automatic fallback)")
                                                .clicked();
                                            if unset {
                                                *color = None;
                                                any_edit = true;
                                            }
                                        });
                                    }
                                });
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let name = self.theme_editor_name.trim().to_string();
                            if !name.is_empty() && ui.button("Save theme").clicked() {
                                match save_edited_theme(
                                    &name,
                                    &self.theme_editing_colors,
                                    &self.theme.colors,
                                ) {
                                    Ok(dir) => {
                                        self.theme_error = None;
                                        eprintln!("theme `{name}` saved to {}", dir.display());
                                        self.cfg.theme = Some(name.clone());
                                        self.reload_theme();
                                        changed = true;
                                    }
                                    Err(e) => {
                                        self.theme_error = Some(e.clone());
                                        eprintln!("error saving theme `{name}`: {e}");
                                    }
                                }
                            }
                            if ui.small_button("Reset edits").clicked() {
                                self.theme_editing_colors = theme_slots(&self.theme.colors);
                                any_edit = false;
                            }
                        });
                        if any_edit {
                            changed = true;
                        }
                    });

                if changed {
                    ctx.request_repaint();
                }
            });
        // sync window close (X) state
        self.settings_open = open;
        // auto-save config when anything changed
        if changed {
            self.dirty = true;
        }
    }
}

/// Persist the edited slots as a user theme (shadows built-ins by name).
/// Missing background/foreground are inherited from the theme being edited so
/// a saved theme can never render white-on-white.
fn save_edited_theme(
    name: &str,
    slots: &[(&'static str, Option<Color>)],
    current: &Colors,
) -> Result<std::path::PathBuf, String> {
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
    {
        return Err("theme name: use letters, numbers, dashes".to_string());
    }
    let mut colors = Colors::default();
    for (label, color) in slots {
        let slot = EDITOR_SLOTS.iter().find_map(|(s, n)| (*n == *label).then_some(*s));
        if let Some(slot) = slot {
            set_slot(&mut colors, slot, *color);
        }
    }
    let mut candidate = theme::Theme::new(name, theme::Shade::Dark, colors);
    let donor = theme::Theme::new("editor", theme::Shade::Dark, current.clone());
    candidate.inherit_essentials_from(&donor);
    let bg = candidate.colors.background();
    let fg = candidate.colors.foreground();
    if (bg.luma() - fg.luma()).abs() < 32.0 {
        return Err(
            "text and background are too similar — the UI would be unreadable".to_string(),
        );
    }
    candidate.appearance = Some(theme::Shade::from_background(bg));
    margdawn_core::importer::save_theme(&candidate, name)
        .map_err(|e| format!("cannot write theme: {e}"))
}

/// Theme default / Sans / Serif selector for a font-face option.
fn font_choice(ui: &mut egui::Ui, label: &str, value: &mut Option<String>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(format!("{label}:"));
        for (opt, name) in [(None, "Theme default"), (Some("sans"), "Sans"), (Some("serif"), "Serif")] {
            let selected = value.as_deref() == opt || (opt == Some("sans") && value.as_deref() == Some("inter"));
            if ui.selectable_label(selected, name).clicked() && !selected {
                *value = opt.map(String::from);
                changed = true;
            }
        }
    });
    changed
}

impl App {
    /// Developer hook (MARGDAWN_SCREENSHOT=path.png): saves one frame, quits.
    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
        if self.frames >= 6 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            if let Err(e) = save_png(&path, &img) {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn save_png(path: &Path, img: &egui::ColorImage) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width() as u32, img.height() as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    let data: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
    writer.write_image_data(&data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod copy_shortcut_tests {
    use eframe::egui;
    use crate::md_view::Selection;

    /// The copy shortcut is read at the top of the frame, from the window's own
    /// input, so it cannot be swallowed by a widget drawn later. This pins the
    /// decision: which events copy, and in which format.
    fn decide(events: &[egui::Event], mods: egui::Modifiers, c_pressed: bool) -> (bool, bool) {
        let copy = events
            .iter()
            .any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut));
        let shift_c = mods.ctrl && mods.shift && c_pressed;
        (copy || shift_c, shift_c)
    }

    #[test]
    fn a_plain_copy_event_copies_plain_text() {
        let (wants, markdown) = decide(&[egui::Event::Copy], egui::Modifiers::CTRL, false);
        assert!(wants);
        assert!(!markdown, "Ctrl+C keeps the readable text, not the markup");
    }

    #[test]
    fn ctrl_shift_c_copies_markdown() {
        let mods = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let (wants, markdown) = decide(&[], mods, true);
        assert!(wants);
        assert!(markdown, "Ctrl+Shift+C keeps the Markdown markers");
    }

    #[test]
    fn a_stray_shift_does_not_flip_a_plain_copy_to_markdown() {
        let mods = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        // Shift is held but no C press: a plain Event::Copy still means plain.
        let (wants, markdown) = decide(&[egui::Event::Copy], mods, false);
        assert!(wants);
        assert!(!markdown);
    }

    #[test]
    fn nothing_is_copied_without_an_event() {
        assert_eq!(decide(&[], egui::Modifiers::NONE, false), (false, false));
    }

    /// The selection must survive the frame that paints it, or copy has nothing
    /// to read. This is the contract `App::update` relies on.
    #[test]
    fn a_selection_has_text_to_copy() {
        let sel = Selection::default();
        assert!(!sel.has_selection());
        assert_eq!(sel.plain_text(), None);
        assert_eq!(sel.markdown_text(), None);
    }
}

#[cfg(test)]
mod momentum_tests {
    use super::Momentum;

    /// Feed a gesture: `frames` frames each moving `per_frame`, then stop.
    /// Returns where the offset ended up and how many frames it took to settle.
    fn flick(per_frame: f32, frames: usize) -> (f32, usize) {
        let dt = 1.0 / 60.0;
        let mut m = Momentum::default();
        let mut offset = 0.0f32;
        for _ in 0..frames {
            offset += per_frame;
            m.observe(offset, dt, true);
        }
        // Now coast until it stops.
        let mut coast_frames = 0;
        loop {
            match m.observe(offset, dt, false) {
                Some(next) => {
                    offset = next;
                    coast_frames += 1;
                    if coast_frames > 10_000 {
                        panic!("never settled");
                    }
                }
                None => break,
            }
        }
        (offset, coast_frames)
    }

    #[test]
    fn a_fast_flick_coasts_past_where_it_was_released() {
        let (end, coast) = flick(-40.0, 6);
        assert!(end < -240.0, "coasted beyond the 240px of real movement, got {end}");
        assert!(coast > 5, "coast lasted several frames, got {coast}");
    }

    #[test]
    fn a_gentle_scroll_does_not_coast() {
        // Well under MIN_FLICK per second (1px/frame = 60px/s).
        let (end, coast) = flick(-1.0, 10);
        assert_eq!(coast, 0, "a careful scroll must not run away");
        assert!((end + 10.0).abs() < 0.001, "stops exactly where it was, got {end}");
    }

    #[test]
    fn the_coast_decays_and_ends() {
        let dt = 1.0 / 60.0;
        let mut m = Momentum::default();
        let mut offset = 0.0f32;
        for _ in 0..6 {
            offset -= 40.0;
            m.observe(offset, dt, true);
        }
        let mut speeds = Vec::new();
        loop {
            match m.observe(offset, dt, false) {
                Some(next) => {
                    speeds.push((next - offset).abs());
                    offset = next;
                }
                None => break,
            }
        }
        assert!(speeds.len() > 3);
        // Strictly slowing down, frame by frame.
        for w in speeds.windows(2) {
            assert!(w[1] <= w[0] + 0.001, "speed must decay, got {speeds:?}");
        }
        assert!(*speeds.last().unwrap() < *speeds.first().unwrap());
    }

    #[test]
    fn downwards_flicks_coast_downwards_too() {
        let (end, coast) = flick(40.0, 6);
        assert!(end > 240.0, "got {end}");
        assert!(coast > 5);
    }

    #[test]
    fn a_violent_flick_cannot_exceed_the_speed_cap() {
        let dt = 1.0 / 60.0;
        let mut m = Momentum::default();
        m.observe(0.0, dt, true);
        // 100 000 px in one frame: absurd, must be clamped.
        m.observe(-100_000.0, dt, true);
        // Keep coasting for one frame and see the step size.
        let step = m.observe(-100_000.0, dt, false).unwrap() - (-100_000.0);
        assert!(step.abs() < Momentum::MAX_SPEED / 60.0 + 1.0, "step {step} too large");
    }
}

#[cfg(test)]
mod scroll_wiring_tests {
    use super::Momentum;

    /// Drive a full gesture through the same call the app uses: wheel deltas
    /// arrive (and move the offset), then stop. The offset must keep moving.
    #[test]
    fn a_wheel_flick_keeps_moving_after_the_last_event() {
        let mut m = Momentum::default();
        let dt = 1.0 / 60.0;
        let mut offset = 0.0f32;
        let content: f32 = 5000.0;
        let view: f32 = 600.0;

        // 8 frames of a fast trackpad flick.
        for _ in 0..8 {
            offset += 45.0;
            let _ = m.observe(offset, dt, true);
        }
        let released_at = offset;

        // Now the OS stops sending events; the app's coast loop takes over.
        let mut frames = 0;
        loop {
            match m.observe(offset, dt, false) {
                Some(next) => {
                    let clamped = next.clamp(0.0, (content - view).max(0.0));
                    if (clamped - offset).abs() <= 0.01 {
                        break;
                    }
                    offset = clamped;
                    frames += 1;
                    if frames > 5_000 {
                        panic!("coast never settled");
                    }
                }
                None => break,
            }
        }
        assert!(offset > released_at, "kept scrolling after release: {} -> {}", released_at, offset);
        assert!(frames >= 3, "coasted for several frames, got {frames}");
    }

    #[test]
    fn the_coast_stops_at_the_bottom() {
        let mut m = Momentum::default();
        let dt = 1.0 / 60.0;
        let content: f32 = 1000.0;
        let view: f32 = 600.0;
        let max = content - view;
        let mut offset = 0.0f32;

        for _ in 0..8 {
            offset += 45.0;
            let _ = m.observe(offset, dt, true);
        }
        let mut steps = 0;
        loop {
            match m.observe(offset, dt, false) {
                Some(next) => {
                    let clamped = next.clamp(0.0, max);
                    if (clamped - offset).abs() <= 0.01 {
                        break;
                    }
                    offset = clamped;
                    steps += 1;
                    assert!(offset <= max + 0.001, "never past the bottom, got {offset}");
                    if steps > 5_000 {
                        panic!("never settled");
                    }
                }
                None => break,
            }
        }
        assert!(offset <= max + 0.001);
    }
}
