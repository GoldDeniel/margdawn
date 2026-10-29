//! eframe app: file loading, theming, settings panel, markdown view.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use margdawn_core::config::{self, Config};
use margdawn_core::md::{parse, Document};
use margdawn_core::theme::{self, Color, ColorKey, Theme};

use crate::emoji::EmojiStore;
use crate::md_view::{RenderCfg, View};

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
    /// developer hook: `MARGDAWN_SCREENSHOT=out.png` saves a frame and quits
    screenshot: Option<PathBuf>,
    frames: u32,
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
            screenshot: std::env::var_os("MARGDAWN_SCREENSHOT").map(PathBuf::from),
            frames: 0,
            cfg,
        };
        if std::env::var_os("MARGDAWN_SETTINGS").is_some() {
            app.settings_open = true;
        }
        if let Some(f) = file {
            app.load_file(&f);
        }
        app
    }

    fn load_file(&mut self, path: &Path) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                self.doc = Some(parse(&text));
                self.path = Some(path.to_path_buf());
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("cannot read {}: {e}", path.display()));
                self.doc = None;
            }
        }
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
        let title = match &self.path {
            Some(p) => format!(
                "{} — margdawn",
                p.file_name().and_then(|n| n.to_str()).unwrap_or("margdawn")
            ),
            None => "margdawn — markdown viewer".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // drag & drop
        let dropped: Vec<egui::DroppedFile> = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.iter().rev().find_map(|f| f.path.clone()) {
            self.load_file(&f);
        }

        let ctrl_o = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::O));

        // ---- top bar --------------------------------------------------------
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Open").clicked() || ctrl_o {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                        .pick_file()
                    {
                        self.load_file(&path);
                    }
                }
                if self.path.is_some() && ui.button("Reload").clicked() {
                    if let Some(p) = self.path.clone() {
                        self.load_file(&p);
                    }
                }
                if ui.button("Settings").clicked() {
                    self.settings_open = !self.settings_open;
                    if self.settings_open {
                        self.theme_editor_name = self.cfg.theme_name().to_string();
                        self.theme_editing_colors = theme_slots(&self.theme.colors);
                    }
                }
                if let Some(p) = &self.path {
                    ui.weak(p.display().to_string());
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
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut view = View::new(
                                    Arc::clone(&self.theme),
                                    RenderCfg::from_config(&self.cfg, &self.theme),
                                    Rc::clone(&self.emoji),
                                );
                                view.render_document(ui, doc);
                            });
                    }
                }
            });

        // ---- settings window --------------------------------------------------
        if self.settings_open {
            self.show_settings(ctx);
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
                let mixed: Vec<String> = user_themes
                    .iter()
                    .cloned()
                    .chain(builtin_themes.iter().map(|s| s.to_string()))
                    .collect();
                let mut sel = self.cfg.theme_name().to_string();
                egui::ComboBox::from_label("Theme")
                    .selected_text(sel.clone())
                    .show_ui(ui, |ui| {
                        for t in &mixed {
                            ui.selectable_value(&mut sel, t.clone(), t);
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
        ctx.request_repaint();
        if self.frames == 12 {
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
