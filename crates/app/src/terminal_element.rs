//! Custom GPUI element that paints the terminal grid: one background quad per
//! non-default cell, a block cursor quad, and a shaped glyph run per row.
//!
//! The grid-walk (span grouping, inverse/cursor color swaps) mirrors
//! `render::build_frame`; only the paint calls target GPUI instead of glyphon.

use std::sync::{Arc, Mutex};

use core_vt::alacritty_terminal::term::cell::Flags;
use core_vt::alacritty_terminal::vte::ansi::CursorShape;
use core_vt::{DEFAULT_BG, DEFAULT_FG, Vt, resolve_color};
use gpui::{
    App, Bounds, Element, Font, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Rgba, Style, TextRun, Window, fill, font, point, px, relative, size,
};

const FONT_SIZE: Pixels = px(14.0);
const LINE_HEIGHT_FACTOR: f32 = 1.25;

pub struct TerminalElement {
    vt: Arc<Mutex<Vt>>,
    font: Font,
    bold_font: Font,
}

impl TerminalElement {
    pub fn new(vt: Arc<Mutex<Vt>>) -> Self {
        let font = font("monospace");
        let mut bold_font = font.clone();
        bold_font.weight = FontWeight::BOLD;
        Self {
            vt,
            font,
            bold_font,
        }
    }
}

/// Accumulates one grid row's text and its style runs during the grid walk.
#[derive(Default)]
struct Row {
    text: String,
    runs: Vec<TextRun>,
}

impl Row {
    fn push(&mut self, c: char, color: Hsla, bold: bool, base: &Font, bold_font: &Font) {
        self.text.push(c);
        let len = c.len_utf8();
        match self.runs.last_mut() {
            // Bold is encoded in the font weight, so comparing color + weight is
            // enough to decide whether the glyph extends the current run.
            Some(run) if run.color == color && (run.font.weight == FontWeight::BOLD) == bold => {
                run.len += len;
            }
            _ => self.runs.push(TextRun {
                len,
                font: if bold {
                    bold_font.clone()
                } else {
                    base.clone()
                },
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }),
        }
    }
}

fn to_hsla(c: [u8; 3]) -> Hsla {
    Rgba {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let font_id = window.text_system().resolve_font(&self.font);
        let Ok(cell_width) = window.text_system().em_advance(font_id, FONT_SIZE) else {
            return;
        };
        let line_height = FONT_SIZE * LINE_HEIGHT_FACTOR;
        let cell_size = size(cell_width, line_height);
        let origin = bounds.origin;

        let vt = self.vt.lock().expect("vt lock");
        let content = vt.term().renderable_content();
        let cursor_visible = content.cursor.shape != CursorShape::Hidden;
        let cursor_point = content.cursor.point;
        let at_bottom = content.display_offset == 0;

        let mut rows: Vec<Row> = Vec::new();
        // Background/cursor quads, collected during the walk and painted before
        // the glyphs so text lands on top.
        let mut quads: Vec<(Point<Pixels>, Hsla)> = Vec::new();

        for indexed in content.display_iter {
            let cell_point = indexed.point;
            if cell_point.line.0 < 0 {
                continue;
            }
            let cell = &indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = resolve_color(cell.fg, content.colors);
            let mut bg = resolve_color(cell.bg, content.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cursor_visible && at_bottom && cell_point == cursor_point {
                // Block cursor: fill the cell with fg, flip the glyph to bg.
                bg = DEFAULT_FG;
                fg = DEFAULT_BG;
            }

            let row = cell_point.line.0 as usize;
            let col = cell_point.column.0;

            if bg != DEFAULT_BG {
                let cell_origin = point(
                    origin.x + cell_width * col as f32,
                    origin.y + line_height * row as f32,
                );
                quads.push((cell_origin, to_hsla(bg)));
            }

            if cell.flags.contains(Flags::DIM) {
                fg = fg.map(|v| (v as f32 * 0.66) as u8);
            }
            let ch = if cell.c == '\t' { ' ' } else { cell.c };
            while rows.len() <= row {
                rows.push(Row::default());
            }
            rows[row].push(
                ch,
                to_hsla(fg),
                cell.flags.contains(Flags::BOLD),
                &self.font,
                &self.bold_font,
            );
        }

        for (cell_origin, color) in quads {
            window.paint_quad(fill(Bounds::new(cell_origin, cell_size), color));
        }

        for (row_idx, row) in rows.into_iter().enumerate() {
            if row.text.is_empty() {
                continue;
            }
            let line_origin = point(origin.x, origin.y + line_height * row_idx as f32);
            let shaped = window.text_system().shape_line(
                row.text.into(),
                FONT_SIZE,
                &row.runs,
                Some(cell_width),
            );
            let _ = shaped.paint(line_origin, line_height, window, cx);
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
