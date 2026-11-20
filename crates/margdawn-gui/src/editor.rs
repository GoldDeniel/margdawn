//! Edit-mode syntax highlighting.
//!
//! The editor is not a rich-text box: it is a plain `TextEdit` over the real
//! source, and this module chooses how each run of characters looks. That keeps
//! the cursor, selection and undo history on the actual file bytes, while the
//! markup (`#`, `**`, `-`, backticks) stays visible — dimmed, so the prose reads
//! as prose and the syntax reads as syntax.
//!
//! Two invariants matter and are covered by tests:
//!
//! 1. **Every character is emitted exactly once, in order.** The `LayoutJob`
//!    text must equal the input byte for byte, or the editor's cursor mapping
//!    drifts and the file gets corrupted on save.
//! 2. **Nothing inside a fenced code block or `$…$` is interpreted.** Math is
//!    opaque by design: a `*` or `_` between dollars is a character, not
//!    emphasis.

use std::sync::Arc;

use eframe::egui::{
    text::{LayoutJob, TextFormat, TextWrapping},
    Color32, FontId, Galley, Response, ScrollArea, TextBuffer, TextEdit,
};

/// Colours and type scale for the editor, resolved from the active theme.
#[derive(Debug, Clone)]
pub struct SyntaxStyle {
    pub fg: Color32,
    pub muted: Color32,
    pub heading: Color32,
    pub code: Color32,
    pub link: Color32,
    pub strong: Color32,
    pub emphasis: Color32,
    pub strike: Color32,
    /// Body font of the editor (monospace); headings scale from this.
    pub font: FontId,
}

impl SyntaxStyle {
    fn base(&self) -> TextFormat {
        TextFormat {
            font_id: self.font.clone(),
            color: self.fg,
            ..Default::default()
        }
    }

    fn dim(&self) -> TextFormat {
        TextFormat {
            font_id: self.font.clone(),
            color: self.muted,
            ..Default::default()
        }
    }

    fn code(&self) -> TextFormat {
        TextFormat {
            font_id: self.font.clone(),
            color: self.code,
            ..Default::default()
        }
    }

    fn heading(&self, level: u8) -> TextFormat {
        let scale = match level {
            1 => 1.6,
            2 => 1.35,
            3 => 1.2,
            4 => 1.1,
            5 => 1.05,
            _ => 1.0,
        };
        TextFormat {
            font_id: FontId::new(self.font.size * scale, self.font.family.clone()),
            color: self.heading,
            ..Default::default()
        }
    }
}

/// Build the styled layout for `src`, wrapping at `wrap_width`.
pub fn highlight(src: &str, st: &SyntaxStyle, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap = TextWrapping {
        max_width: wrap_width,
        ..Default::default()
    };
    // LayoutJob is append-only and keeps its own text; every push must be the
    // next contiguous slice of `src`.
    let mut pos = 0usize;
    let mut fence: Option<[u8; 3]> = None;

    for line in src.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let has_nl = line.ends_with('\n');
        let body = if has_nl { &line[..line.len() - 1] } else { line };
        let body_end = start + body.len();
        let nl_end = start + line.len();

        let trimmed = body.trim_start();
        let indent = body.len() - trimmed.len();

        // ---- fenced code -------------------------------------------------
        if let Some(marker) = fence {
            let closes = trimmed.starts_with(std::str::from_utf8(&marker).unwrap_or("```"))
                && trimmed.trim_matches(|c| c == marker_char(marker)).trim().is_empty();
            if closes {
                // the closing fence itself is syntax, not code
                push(&mut job, src, start, body_end, &st.dim());
                fence = None;
            } else {
                push(&mut job, src, start, body_end, &st.code());
            }
        } else if let Some(m) = fence_open(trimmed) {
            fence = Some(m);
            push(&mut job, src, start, body_end, &st.dim());
        }
        // ---- headings ----------------------------------------------------
        else if let Some((level, content_at)) = heading(body) {
            push(&mut job, src, start, start + content_at, &st.dim());
            let base = st.heading(level);
            inline(&mut job, src, start + content_at, body_end, &base, st);
        }
        // ---- block quote --------------------------------------------------
        else if trimmed.starts_with('>') {
            let q_end = start + indent + quote_prefix_len(trimmed);
            push(&mut job, src, start, q_end, &st.dim());
            inline(&mut job, src, q_end, body_end, &st.dim(), st);
        }
        // ---- thematic break -----------------------------------------------
        else if is_rule(trimmed) {
            push(&mut job, src, start, body_end, &st.dim());
        }
        // ---- list item -----------------------------------------------------
        else if let Some(off) = list_marker(trimmed) {
            let m_end = start + indent + off;
            push(&mut job, src, start, m_end, &st.dim());
            inline(&mut job, src, m_end, body_end, &st.base(), st);
        }
        // ---- plain paragraph ------------------------------------------------
        else {
            inline(&mut job, src, start, body_end, &st.base(), st);
        }

        if has_nl {
            push(&mut job, src, body_end, nl_end, &st.base());
        }
    }

    job
}

fn marker_char(m: [u8; 3]) -> char {
    m[0] as char
}

/// What the editable pane produced in one frame.
#[derive(Debug)]
pub struct EditorOutput {
    /// the underlying `TextEdit` (`.changed()`, `.id`, focus)
    pub response: Response,
    /// vertical scroll offset the pane sits at this frame (0.0 = top).
    /// Read by the scroll regression tests, which is why it is not private.
    #[cfg_attr(not(test), allow(dead_code))]
    pub scroll_offset: f32,
}

/// The editable source pane: `text` styled by `highlight`, in a vertical
/// scroll area at least `min_rows` rows tall.
///
/// The scroll area is load-bearing, not decoration. `TextEdit` has no
/// scrolling of its own — it sizes itself to the whole galley — and egui only
/// pans to the caret when a parent `ScrollArea` exists (`TextEdit` calls
/// `ui.scroll_to_rect` on every edit). Dropped straight into a panel, a
/// document taller than the window cannot be scrolled at all: everything past
/// the panel is clipped away, and the caret is freely paintable outside the
/// visible rect. `min_rows` fills the viewport so clicking the blank space
/// under a short document still puts the caret at the end.
pub fn source_editor(
    ui: &mut eframe::egui::Ui,
    text: &mut dyn TextBuffer,
    st: &SyntaxStyle,
    min_rows: usize,
) -> EditorOutput {
    let mut layouter = |ui: &eframe::egui::Ui, text: &str, wrap_width: f32| -> Arc<Galley> {
        let job = highlight(text, st, wrap_width);
        ui.fonts(|f| f.layout_job(job))
    };

    let edit = TextEdit::multiline(text)
        .font(st.font.clone())
        .desired_width(f32::INFINITY)
        .desired_rows(min_rows.max(1))
        .frame(false)
        .margin(eframe::egui::Margin::same(0.0))
        .lock_focus(true)
        .layouter(&mut layouter);

    let out = ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| ui.add(edit));
    EditorOutput {
        response: out.inner,
        scroll_offset: out.state.offset.y,
    }
}

/// Write `text` to `path` without ever leaving a half-written file behind.
///
/// `std::fs::write` truncates first, so a crash or a full disk mid-write eats
/// the user's file. Instead: write a sibling temp file, copy the original's
/// permission bits onto it, then rename over the target (atomic on the same
/// filesystem). Symlinks are resolved first so the link is not replaced by a
/// regular file.
pub fn save_text(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;

    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "margdawn.md".to_string());
    let tmp = dir.join(format!(".{name}.margdawn-{}.tmp", std::process::id()));

    let existing_perms = std::fs::metadata(&target).ok().map(|m| m.permissions());
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    if let Some(perms) = existing_perms {
        let _ = std::fs::set_permissions(&tmp, perms);
    }
    match std::fs::rename(&tmp, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn push(job: &mut LayoutJob, src: &str, a: usize, b: usize, f: &TextFormat) {
    if a >= b || b > src.len() {
        return;
    }
    if let Some(s) = src.get(a..b) {
        job.append(s, 0.0, f.clone());
    }
}

/// ```` ``` ```` or `~~~` with optional info string.
fn fence_open(trimmed: &str) -> Option<[u8; 3]> {
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 {
        let c = bytes[0];
        if (c == b'`' || c == b'~') && bytes[1] == c && bytes[2] == c {
            return Some([c, c, c]);
        }
    }
    None
}

/// `# Title` → (level, byte offset where the content starts).
fn heading(body: &str) -> Option<(u8, usize)> {
    let bytes = body.as_bytes();
    let mut n = 0;
    while n < bytes.len() && bytes[n] == b'#' && n < 6 {
        n += 1;
    }
    if n == 0 {
        return None;
    }
    // ATX headings need a space after the hashes (or end of line).
    if bytes.len() == n {
        return Some((n as u8, n));
    }
    if bytes[n] == b' ' {
        let mut at = n;
        while at < bytes.len() && bytes[at] == b' ' {
            at += 1;
        }
        return Some((n as u8, at));
    }
    None
}

/// Length of `> ` / `>> ` style prefixes.
fn quote_prefix_len(trimmed: &str) -> usize {
    let mut i = 0;
    let b = trimmed.as_bytes();
    while i < b.len() && (b[i] == b'>' || b[i] == b' ') {
        i += 1;
    }
    i
}

fn is_rule(trimmed: &str) -> bool {
    let t = trimmed.trim_end();
    if t.len() < 3 {
        return false;
    }
    for ch in ['-', '*', '_'] {
        if t.starts_with(ch) {
            let only = t.chars().all(|c| c == ch || c == ' ');
            let count = t.chars().filter(|c| *c == ch).count();
            if only && count >= 3 {
                return true;
            }
        }
    }
    false
}

/// `- `, `* `, `+ `, `1. `, `1) ` — returns the byte length of the marker run.
fn list_marker(trimmed: &str) -> Option<usize> {
    let b = trimmed.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+') && b[1] == b' ' {
        return Some(2);
    }
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i + 1 < b.len() && matches!(b[i], b'.' | b')') && b[i + 1] == b' ' {
        return Some(i + 2);
    }
    None
}

/// Inline markup inside `[a, b)`: emphasis, code spans, links, math.
fn inline(
    job: &mut LayoutJob,
    src: &str,
    a: usize,
    b: usize,
    base: &TextFormat,
    st: &SyntaxStyle,
) {
    if a >= b {
        return;
    }
    let Some(text) = src.get(a..b) else { return };
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut plain_from = 0usize;

    // flush [plain_from, i) as base text
    macro_rules! flush {
        ($upto:expr) => {
            if plain_from < $upto {
                push(job, src, a + plain_from, a + $upto, base);
            }
        };
    }

    while i < bytes.len() {
        let c = bytes[i];
        let mut handled = false;

        // ---- code span ----------------------------------------------------
        if c == b'`' {
            if let Some(k) = find_byte(bytes, i + 1, b'`') {
                flush!(i);
                push(job, src, a + i, a + i + 1, &st.dim());
                push(job, src, a + i + 1, a + k, &st.code());
                push(job, src, a + k, a + k + 1, &st.dim());
                i = k + 1;
                plain_from = i;
                handled = true;
            }
        }

        // ---- math: `$…$` / `$$…$$`, opaque, never re-formatted -------------
        if !handled && c == b'$' {
            let delim: &[u8] = if bytes.get(i + 1) == Some(&b'$') { b"$$" } else { b"$" };
            if let Some(k) = find_seq(bytes, i + delim.len(), delim) {
                flush!(i);
                push(job, src, a + i, a + i + delim.len(), &st.dim());
                push(job, src, a + i + delim.len(), a + k, &st.code());
                push(job, src, a + k, a + k + delim.len(), &st.dim());
                i = k + delim.len();
                plain_from = i;
                handled = true;
            }
        }

        // ---- link [text](url) ---------------------------------------------
        if !handled && c == b'[' {
            if let Some((label_end, url_end)) = link_extent(bytes, i) {
                flush!(i);
                push(job, src, a + i, a + i + 1, &st.dim()); // [
                let link_fmt = TextFormat {
                    underline: eframe::egui::Stroke::new(1.0_f32, st.link),
                    color: st.link,
                    ..base.clone()
                };
                push(job, src, a + i + 1, a + label_end, &link_fmt);
                push(job, src, a + label_end, a + label_end + 2, &st.dim()); // ](
                push(job, src, a + label_end + 2, a + url_end - 1, &st.dim()); // url
                push(job, src, a + url_end - 1, a + url_end, &st.dim()); // )
                i = url_end;
                plain_from = i;
                handled = true;
            }
        }

        // ---- strong / strike (2 chars) / emphasis (1 char) -----------------
        if !handled && matches!(c, b'*' | b'_' | b'~') {
            let double = bytes.get(i + 1) == Some(&c);
            let n = if double { 2 } else { 1 };
            // `~` is only meaningful doubled
            if c != b'~' || double {
                if let Some(k) = find_run(bytes, i + n, c, n) {
                    flush!(i);
                    push(job, src, a + i, a + i + n, &st.dim());
                    let mut inner = base.clone();
                    match (c, n) {
                        (b'~', 2) => {
                            inner.strikethrough = eframe::egui::Stroke::new(1.0_f32, st.strike);
                        }
                        (_, 2) => {
                            inner.color = st.strong;
                            if base.color != st.heading {
                                inner.color = st.strong;
                            }
                        }
                        (_, _) => {
                            inner.italics = true;
                            inner.color = st.emphasis;
                        }
                    }
                    // markers inside (rare, e.g. `**bold and _italic_**`) stay plain
                    push(job, src, a + i + n, a + k, &inner);
                    push(job, src, a + k, a + k + n, &st.dim());
                    i = k + n;
                    plain_from = i;
                }
            }
        }

        if !handled {
            i += 1;
        }
    }
    flush!(bytes.len());
    let _ = job; // (kept for symmetry with the macro)
}

fn find_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    bytes.iter().position(|&b| b == needle).filter(|&p| p >= from)
}

fn find_seq(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// A run of exactly `n` characters `c`, not part of a longer run.
fn find_run(bytes: &[u8], from: usize, c: u8, n: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == c && bytes[i..].iter().take_while(|b| **b == c).count() >= n {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `[label](url)` → (index of `]`, index just past `)`).
fn link_extent(bytes: &[u8], open: usize) -> Option<(usize, usize)> {
    let close = find_byte(bytes, open + 1, b']')?;
    if bytes.get(close + 1) != Some(&b'(') {
        return None;
    }
    let paren = find_byte(bytes, close + 2, b')')?;
    Some((close, paren + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> SyntaxStyle {
        let c = |v: u8| Color32::from_rgb(v, v, v);
        SyntaxStyle {
            fg: c(1),
            muted: c(2),
            heading: c(3),
            code: c(4),
            link: c(5),
            strong: c(6),
            emphasis: c(7),
            strike: c(8),
            font: FontId::monospace(14.0),
        }
    }

    fn styled(src: &str) -> LayoutJob {
        highlight(src, &style(), 600.0)
    }

    /// The one invariant that can silently corrupt a file.
    #[test]
    fn text_round_trips_exactly() {
        let cases = [
            "",
            "plain\n",
            "# Title\n\nbody **bold** and *it* and `code`\n",
            "- a\n- b\n  - c\n",
            "> quoted **text**\n",
            "```rust\nfn main() { println!(\"# not a heading\"); }\n```\n",
            "[link](https://example.com) and $x^2 * y$ tail\n",
            "unmatched * star and $ dollar and `tick\n",
            "~~~~\ncode\n~~~~\n",
            "trailing space   \r\nnext\r\n",
        ];
        for src in cases {
            let job = styled(src);
            assert_eq!(job.text, src, "layout text drifted from source: {src:?}");
        }
    }

    #[test]
    fn heading_marks_are_dimmed_and_content_is_scaled() {
        let job = styled("## Hello\n");
        let hashes = &job.sections[0];
        assert_eq!(&job.text[hashes.byte_range.clone()], "## ");
        assert_eq!(hashes.format.color, Color32::from_rgb(2, 2, 2));
        let content = &job.sections[1];
        assert_eq!(&job.text[content.byte_range.clone()], "Hello");
        assert_eq!(content.format.color, Color32::from_rgb(3, 3, 3));
        assert!(content.format.font_id.size > 14.0, "heading should be larger");
    }

    #[test]
    fn fenced_code_is_never_interpreted() {
        let src = "```\n# not a heading\n**not bold**\n```\n";
        let job = styled(src);
        assert_eq!(job.text, src);
        // every section inside the fence is code-coloured, none heading-coloured
        let mut i = 0;
        for sec in &job.sections {
            let slice = &job.text[sec.byte_range.clone()];
            if slice.contains("not a heading") || slice.contains("not bold") {
                assert_eq!(sec.format.color, Color32::from_rgb(4, 4, 4), "{slice:?}");
                i += 1;
            }
        }
        assert!(i >= 2, "the code lines should have been found");
    }

    #[test]
    fn math_is_opaque() {
        let src = "energy is $E = m * c^2$ here\n";
        let job = styled(src);
        assert_eq!(job.text, src);
        // the `*` inside dollars must not start emphasis: the run containing it
        // keeps the code colour
        let run = job
            .sections
            .iter()
            .find(|s| job.text[s.byte_range.clone()].contains('*'))
            .expect("math run");
        assert_eq!(run.format.color, Color32::from_rgb(4, 4, 4));
        assert!(!run.format.italics);
    }

    #[test]
    fn unmatched_dollar_stays_literal() {
        let src = "costs $5 and nothing closes\n";
        let job = styled(src);
        assert_eq!(job.text, src);
        // the `$` must be plain text, not an opener: everything from it on keeps
        // the base colour (a mismatched `$` must never swallow the line)
        let at = src.find("$5").unwrap();
        for sec in &job.sections {
            let r = sec.byte_range.clone();
            if r.start >= at {
                assert_eq!(
                    sec.format.color,
                    Color32::from_rgb(1, 1, 1),
                    "{:?} was styled as something else",
                    &job.text[r]
                );
            }
        }
    }

    #[test]
    fn emphasis_is_styled() {
        let job = styled("a **b** c\n");
        let bold = job
            .sections
            .iter()
            .find(|s| &job.text[s.byte_range.clone()] == "b")
            .expect("bold run");
        assert_eq!(bold.format.color, Color32::from_rgb(6, 6, 6));
        let marker = &job.sections[0];
        assert_eq!(&job.text[marker.byte_range.clone()], "a ");
    }

    #[test]
    fn empty_fence_does_not_panic() {
        let job = styled("```\n");
        assert_eq!(job.text, "```\n");
    }

    // ---- the pane must scroll ----------------------------------------------

    /// One real egui frame with the editor pane in a full-window panel.
    fn pane_frame(
        ctx: &eframe::egui::Context,
        input: eframe::egui::RawInput,
        text: &mut String,
        st: &SyntaxStyle,
    ) -> (eframe::egui::Id, f32) {
        use eframe::egui;
        let mut got = None;
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let row_h = ui.fonts(|f| f.row_height(&st.font));
                let rows = ((ui.available_height() / row_h).ceil() as usize).max(1);
                let out = source_editor(ui, text, st, rows);
                got = Some((out.response.id, out.scroll_offset));
            });
        });
        got.expect("the editor pane should have been shown")
    }

    /// Run `events` on the first frame, then idle frames — scroll animations
    /// need a while to settle. Returns the pane's id and final scroll offset.
    fn pane_run(
        ctx: &eframe::egui::Context,
        events: Vec<eframe::egui::Event>,
        frames: usize,
        text: &mut String,
        st: &SyntaxStyle,
    ) -> (eframe::egui::Id, f32) {
        use eframe::egui;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 220.0));
        let mut got = (egui::Id::NULL, 0.0);
        for i in 0..frames.max(1) {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events: if i == 0 { events.clone() } else { Vec::new() },
                ..Default::default()
            };
            got = pane_frame(ctx, input, text, st);
        }
        got
    }

    /// Click at `pos`: press on one frame, release on the next. This is what a
    /// real pointer does, and what gives the editor focus plus a caret.
    fn pane_click(
        ctx: &eframe::egui::Context,
        pos: eframe::egui::Pos2,
        text: &mut String,
        st: &SyntaxStyle,
    ) -> (eframe::egui::Id, f32) {
        use eframe::egui;
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut got = (egui::Id::NULL, 0.0);
        for ev in [button(true), button(false)] {
            got = pane_run(ctx, vec![egui::Event::PointerMoved(pos), ev], 2, text, st);
        }
        got
    }

    /// The bug this guards: the pane used to be a bare `TextEdit` sized to the
    /// window, so everything past the window was clipped away with no way to
    /// reach it, and the caret could be drawn outside the visible rect.
    #[test]
    fn a_long_document_scrolls_and_the_caret_stays_in_view() {
        use eframe::egui;

        let ctx = egui::Context::default();
        let st = style();
        let mut text: String = (0..300)
            .map(|i| format!("line {i:03} of a document much taller than the window\n"))
            .collect();

        // a fresh pane sits at the top
        let (_, offset) = pane_run(&ctx, Vec::new(), 2, &mut text, &st);
        assert_eq!(offset, 0.0, "a fresh pane should start at the top");

        // wheel down over the pane: the "I can't scroll" move
        let wheel = vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 100.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let (_, after_wheel) = pane_run(&ctx, wheel, 10, &mut text, &st);
        assert!(
            after_wheel > 0.0,
            "the wheel did not move the pane: offset = {after_wheel}"
        );

        // click into the text so the editor has focus and a caret
        let (id, _) = pane_click(&ctx, egui::pos2(60.0, 60.0), &mut text, &st);
        assert!(
            ctx.memory(|m| m.has_focus(id)),
            "the click did not focus the pane"
        );

        // Ctrl+End moves the caret to the last line: the pane has to pan down
        // after it instead of letting the caret sink out of the window
        let key = vec![egui::Event::Key {
            key: egui::Key::End,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        }];
        let (_, at_end) = pane_run(&ctx, key, 60, &mut text, &st);
        assert!(
            at_end > 3000.0,
            "the pane did not follow the caret to the end: offset = {at_end}"
        );
    }

    /// A short document still gets a full-height pane, so clicking the empty
    /// space below the text lands the caret at the end instead of nowhere.
    #[test]
    fn a_short_document_still_fills_the_viewport() {
        use eframe::egui;

        let ctx = egui::Context::default();
        let st = style();
        let mut text = "# tiny\n".to_string();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 220.0));

        // two frames: fonts only exist once `Context::run` has seen a frame
        let mut bottom = 0.0;
        let mut panel_bottom = 0.0;
        let mut row_h = 0.0;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panel_bottom = ui.available_rect_before_wrap().bottom();
                    row_h = ui.fonts(|f| f.row_height(&st.font));
                    let rows = ((ui.available_height() / row_h).ceil() as usize).max(1);
                    let out = source_editor(ui, &mut text, &st, rows);
                    bottom = out.response.rect.bottom();
                });
            });
        }
        // the pane covers every row that fits: the leftover must be less than
        // one more row of empty background
        assert!(
            panel_bottom - bottom < row_h,
            "pane bottom {bottom} left {} of dead space below it (one row is {row_h})",
            panel_bottom - bottom
        );
    }

    // ---- writing back ------------------------------------------------------

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("margdawn-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_writes_exactly_the_buffer() {
        let d = tmpdir("save");
        let p = d.join("doc.md");
        std::fs::write(&p, "# old\n").unwrap();
        let body = "# new\n\n**bold** and $x*y$\n";
        save_text(&p, body).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), body);
        // no temp files left behind
        let leftovers: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left: {leftovers:?}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn save_keeps_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("perms");
        let p = d.join("doc.md");
        std::fs::write(&p, "x\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        save_text(&p, "y\n").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_failed_save_leaves_the_original_intact() {
        let d = tmpdir("fail");
        let p = d.join("doc.md");
        std::fs::write(&p, "original\n").unwrap();
        let missing = d.join("nope").join("doc.md");
        assert!(save_text(&missing, "whatever\n").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "original\n");
        std::fs::remove_dir_all(&d).ok();
    }
}
