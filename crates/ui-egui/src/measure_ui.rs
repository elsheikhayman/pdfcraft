//! Measure objects: page-space clicks, live readings and persistent drawing scales.
use crate::{LeftPanel, PdfCraftApp, QuickTool, canvas::DocView, theme::Tokens, units::Unit, widgets};
use egui::{Color32, Pos2, Stroke};
use pdfcraft_engine::{
    Document, Edit, Style,
    measure::{self, Kind, NewMeasurement, Point, Reading, Scale},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Distance,
    Perimeter,
    Area,
    Calibrate,
}
impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Perimeter => "perimeter",
            Self::Area => "area",
            Self::Calibrate => "calibrate",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "distance" => Some(Self::Distance),
            "perimeter" => Some(Self::Perimeter),
            "area" => Some(Self::Area),
            "calibrate" => Some(Self::Calibrate),
            _ => None,
        }
    }
    fn kind(self) -> Kind {
        match self {
            Self::Area => Kind::Area,
            Self::Perimeter => Kind::Perimeter,
            _ => Kind::Distance,
        }
    }
}
/// Real-world units offered next to the scale: name and millimetres per unit.
const REAL_UNITS: [(&str, f64); 6] = [("mm", 1.0), ("cm", 10.0), ("m", 1000.0), ("km", 1_000_000.0), ("in", 25.4), ("ft", 304.8)];
/// A calibration waiting for the real length: the distance clicked on the page, in view points.
pub struct Calibration {
    measured: f64,
    text: String,
    unit: String,
    error: Option<String>,
}
pub struct MeasureView {
    pub points: Vec<Point>,
    pub page: Option<usize>,
    pub tool: Option<Tool>,
    pub reading: Option<Reading>,
    pub scale: Option<Scale>,
    pub error: Option<String>,
    pub snaps: measure::snap::SnapOptions,
    pub snap_enabled: bool,
    /// Lock new vertices to horizontal/vertical from the last one (F8), like AutoCAD ortho.
    pub ortho: bool,
    pub sensitivity: f64,
    pub drawing_points: f64,
    /// The unit the drawing side of the scale is shown in ("1 mm = 100 cm").
    pub paper_unit: Unit,
    pub real_distance: f64,
    pub calibration: Option<Calibration>,
    /// A scale field changed and is not stored in the page yet; applied once the field is released.
    scale_dirty: bool,
    pub unit: String,
    pub precision: u8,
    /// A scale field changed and is not stored in the page yet; stored once the field is released.
    scale_dirty: bool,
    pub name: String,
    pub whole_page: bool,
    pub rect: [f64; 4],
    pub label: String,
    seeded_page: Option<usize>,
    /// Snapping geometry (or why it failed) per (page, edit generation): extraction is not
    /// repeated on every hover frame.
    paths: Option<(usize, u64, Result<measure::snap::Geometry, String>)>,
    /// Saved measurements per edit generation, for the panel.
    listing: Option<(u64, Result<measure::Listing, String>)>,
}
impl Default for MeasureView {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            page: None,
            tool: None,
            reading: None,
            scale: None,
            error: None,
            snaps: Default::default(),
            snap_enabled: true,
            ortho: false,
            sensitivity: 6.0,
            drawing_points: 72.0,
            paper_unit: Unit::Millimetres,
            real_distance: 1.0,
            calibration: None,
            scale_dirty: false,
            unit: "in".into(),
            precision: 2,
            scale_dirty: false,
            name: "Drawing scale".into(),
            whole_page: true,
            rect: [0.0, 0.0, 100.0, 100.0],
            label: String::new(),
            seeded_page: None,
            paths: None,
            listing: None,
        }
    }
}
impl MeasureView {
    pub fn cancel(&mut self) {
        self.points.clear();
        self.page = None;
        self.reading = None;
        self.scale = None;
    }
}
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    doc: &Document,
    view: &mut DocView,
    cx: &crate::comments::PageCx<'_>,
    tool: Tool,
    units: crate::units::Unit,
) {
    let page = cx.page;
    let xf = cx.xf;
    let author = &cx.prefs.author;
    if !doc.allows_annotation() {
        return;
    }
    let Some(info) = doc.info.pages.get(page) else { return };
    let state = &mut view.measure;
    if state.tool != Some(tool) {
        state.cancel();
        state.tool = Some(tool);
    }
    // Keys belong to a focused text field (the label, unit or viewport name) when there is one.
    let typing = ui.ctx().egui_wants_keyboard_input();
    if !typing && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.cancel();
    }
    if !typing {
        if ui.input(|i| i.key_pressed(egui::Key::F3)) {
            state.snap_enabled = !state.snap_enabled;
        }
        if ui.input(|i| i.key_pressed(egui::Key::F8)) {
            state.ortho = !state.ortho;
        }
    }
    let screen = |p: Point| {
        let v = info.user_to_view(p[0] as f32, p[1] as f32);
        xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
    };
    let pos = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p) && resp.hovered());
    let mut hover = None;
    let mut snap = None;
    let mut constrained = false;
    if let Some(pos) = pos {
        // The crosshair is drawn below, so the system cursor would only double it.
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        let (vx, vy) = xf.screen_to_view(pos);
        let [x, y] = info.view_to_user(vx, vy);
        let mut point = [f64::from(x), f64::from(y)];
        if state.snap_enabled {
            if state.paths.as_ref().is_none_or(|(p, g, _)| *p != page || *g != doc.edit_generation()) {
                state.paths = Some((page, doc.edit_generation(), doc.measurement_paths(page)));
            }
            let pixels_per_unit = (xf.rect.width() / xf.pw).max(1e-6);
            if let Some((_, _, Err(e))) = &state.paths {
                state.error = Some(e.clone());
            }
            if let Some((_, _, Ok(paths))) = &state.paths {
                if paths.truncated {
                    state.error = Some(tl!("This drawing exceeds the snapping limit; only the first paths are used.").into());
                } else if paths.unreadable > 0 {
                    state.error = Some(tl!("Some page content couldn't be read; snapping uses the rest.").into());
                }
                let tolerance = doc
                    .measurement_to_user(page, [f64::from(vx) + state.sensitivity / f64::from(pixels_per_unit), f64::from(vy)])
                    .map(|p| measure::distance(point, p))
                    .unwrap_or(state.sensitivity / f64::from(pixels_per_unit));
                if state.snaps.intersections && paths.intersection_limited(point, tolerance) {
                    state.error = Some(tl!("This area has too many overlapping paths; intersection snapping is limited.").into());
                }
                if let Ok(Some(hit)) = paths.snap(point, tolerance, state.snaps) {
                    point = hit.point;
                    snap = Some(hit.kind);
                }
            }
        }
        let shift = ui.input(|i| i.modifiers.shift);
        if (shift || state.ortho)
            && let Some(last) = state.points.last()
        {
            point = if shift { measure::snap::constrain(*last, point) } else { measure::snap::constrain_ortho(*last, point) };
            constrained = true;
        }
        hover = Some(point);
        if resp.clicked() {
            if state.page != Some(page) {
                state.cancel();
                state.page = Some(page);
            }
            if state.points.is_empty() {
                match doc.measurement_scale_or(page, point, units.per_point(), units.label()) {
                    Ok(scale) => {
                        state.scale = Some(scale);
                        state.error = None;
                    }
                    Err(e) => {
                        state.error = Some(e);
                        return;
                    }
                }
            }
            let closes =
                tool == Tool::Area && state.points.len() >= 3 && state.points.first().is_some_and(|first| screen(*first).distance(pos) <= 6.0);
            let repeats = state.points.last().is_some_and(|last| measure::distance(*last, point) < 1e-6);
            if !closes && !repeats && state.points.len() < measure::MAX_POINTS {
                state.points.push(point);
            }
            let finishes = closes || matches!(tool, Tool::Distance | Tool::Calibrate) && state.points.len() == 2;
            if finishes {
                finish(state, &mut view.pending_edit, doc, page, tool, author);
            }
        }
    }
    let accent = Color32::from_rgb(0, 120, 214);
    let tokens = Tokens::get(ui.ctx());
    let mut drawing = state.page == Some(page) && !state.points.is_empty();
    if drawing {
        if (resp.double_clicked() || !typing && ui.input(|i| i.key_pressed(egui::Key::Enter))) && matches!(tool, Tool::Perimeter | Tool::Area) {
            finish(state, &mut view.pending_edit, doc, page, tool, author);
        }
        if !typing && ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
            state.points.pop();
        }
        drawing = state.page == Some(page) && !state.points.is_empty();
    }
    let mut points = if drawing { state.points.clone() } else { Vec::new() };
    if drawing
        && let Some(p) = hover
        && points.last().is_none_or(|last| measure::distance(*last, p) > 1e-6)
    {
        points.push(p);
    }
    if drawing && let Some(scale) = &state.scale {
        match measure::reading(tool.kind(), &points, scale) {
            Ok(reading) => state.reading = Some(reading),
            Err(e) => state.error = Some(e.to_string()),
        }
    }
    let positions: Vec<Pos2> = points.iter().map(|&p| screen(p)).collect();
    if drawing {
        // Ortho/shift guide: a dashed tracking line through the last vertex and the cursor.
        if constrained && let (Some(last), Some(p)) = (state.points.last(), hover) {
            let (a, b) = (screen(*last), screen(p));
            let d = b - a;
            if d.length() > 1.0 {
                let d = d.normalized() * 4000.0;
                ui.painter().extend(egui::Shape::dashed_line(&[a - d, a + d], Stroke::new(1.0, accent.gamma_multiply(0.8)), 6.0, 4.0));
            }
        }
        let mut lines: Vec<(Vec<Pos2>, bool)> = positions.windows(2).map(|w| (w.to_vec(), false)).collect();
        if tool == Tool::Area
            && positions.len() >= 3
            && let (Some(a), Some(b)) = (positions.first(), positions.last())
        {
            lines.push((vec![*b, *a], false));
        }
        halo_lines(ui, &lines, accent, 2.0);
        // Committed vertices are squares so they read as handles, not as part of the drawing.
        for &p in positions.iter().take(state.points.len()) {
            let r = egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0));
            ui.painter().rect_filled(r, 0.0, accent);
            ui.painter().rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
        }
        // Per-segment lengths, centred on each segment.
        if let Some(scale) = &state.scale {
            let skip = points.len().saturating_sub(65);
            for (i, w) in points.windows(2).enumerate().skip(skip) {
                let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                let Ok(r) = measure::reading(Kind::Distance, &[*a, *b], scale) else { continue };
                let (sa, sb) = (screen(*a), screen(*b));
                let d = sb - sa;
                if d.length() < 24.0 && i + 2 != points.len() {
                    continue;
                }
                let mut n = if d.length() > 1e-3 { egui::vec2(-d.y, d.x).normalized() } else { egui::vec2(0.0, -1.0) };
                if n.y > 0.0 {
                    n = -n;
                }
                chip(ui, sa + d * 0.5 + n * 14.0, egui::Align2::CENTER_CENTER, &r.label, false, tokens.text, tokens.card);
            }
        }
        // Closing hint: the first vertex lights up when a click would close the area.
        if tool == Tool::Area
            && state.points.len() >= 3
            && let (Some(first), Some(pos)) = (positions.first(), pos)
            && first.distance(pos) <= 6.0
        {
            ui.painter().circle_stroke(*first, 10.0, Stroke::new(3.5, Color32::from_black_alpha(190)));
            ui.painter().circle_stroke(*first, 10.0, Stroke::new(1.5, Color32::from_rgb(46, 158, 92)));
            chip(ui, *first + egui::vec2(14.0, -14.0), egui::Align2::LEFT_BOTTOM, tl!("Close"), false, tokens.text, tokens.card);
        }
        // Live total, angle and deltas next to the cursor.
        if let (Some(p), Some(reading), Some(scale)) = (hover, &state.reading, &state.scale) {
            let c = screen(p);
            let precision = usize::from(scale.precision);
            let total = matches!(tool, Tool::Perimeter | Tool::Area);
            if total {
                chip(ui, c + egui::vec2(16.0, 16.0), egui::Align2::LEFT_TOP, &reading.label, true, tokens.accent_text, tokens.accent);
            }
            if let Some(last) = state.points.last() {
                let d = measure::reading(Kind::Distance, &[*last, p], scale).map(|r| (r.angle, r.delta_x, r.delta_y)).unwrap_or_default();
                let text = format!("{:.1}°   Δx {:.precision$}   Δy {:.precision$}", d.0, d.1.abs(), d.2.abs());
                let y = if total { 42.0 } else { 16.0 };
                chip(ui, c + egui::vec2(16.0, y), egui::Align2::LEFT_TOP, &text, false, tokens.text_muted, tokens.card);
            }
        }
    }
    if let Some(pos) = pos {
        crosshair(&ui.painter().with_clip_rect(xf.rect.intersect(ui.clip_rect())), pos);
        if let (Some(p), Some(kind)) = (hover, snap) {
            snap_glyph(ui, screen(p), kind);
            chip(ui, screen(p) + egui::vec2(12.0, -12.0), egui::Align2::LEFT_BOTTOM, snap_name(kind), false, tokens.text, tokens.card);
        } else if state.snap_enabled {
            // Pick box where a snap would be looked for.
            let half = state.sensitivity as f32;
            let r = egui::Rect::from_center_size(pos, egui::vec2(half * 2.0, half * 2.0));
            ui.painter().rect_stroke(r, 0.0, Stroke::new(3.0, Color32::from_black_alpha(120)), egui::StrokeKind::Middle);
            ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(255, 212, 0)), egui::StrokeKind::Middle);
        }
        // Mode strip, like the AutoCAD status bar.
        let clip = ui.clip_rect();
        let on = |b: bool| if b { tl!("on") } else { tl!("off") };
        let status = format!("{} F3 {}   {} F8 {}   Shift 45°", tl!("Snap"), on(state.snap_enabled), tl!("Ortho"), on(state.ortho));
        chip(ui, egui::pos2(clip.left() + 8.0, clip.bottom() - 8.0), egui::Align2::LEFT_BOTTOM, &status, false, tokens.text_muted, tokens.card);
    }
}
/// Half-length of each crosshair arm in screen points.
const CROSSHAIR_ARM: f32 = 90.0;
/// AutoCAD-style crosshair: long thin arms with a halo, readable on light, dark and busy pages.
fn crosshair(painter: &egui::Painter, c: Pos2) {
    let arms = [
        [c - egui::vec2(CROSSHAIR_ARM, 0.0), c + egui::vec2(CROSSHAIR_ARM, 0.0)],
        [c - egui::vec2(0.0, CROSSHAIR_ARM), c + egui::vec2(0.0, CROSSHAIR_ARM)],
    ];
    for stroke in [Stroke::new(3.0, Color32::from_black_alpha(110)), Stroke::new(1.0, Color32::WHITE)] {
        for arm in arms {
            painter.line_segment(arm, stroke);
        }
    }
}
fn snap_name(kind: measure::snap::SnapKind) -> &'static str {
    tl!(match kind {
        measure::snap::SnapKind::Endpoint => "Endpoint",
        measure::snap::SnapKind::Midpoint => "Midpoint",
        measure::snap::SnapKind::Intersection => "Intersection",
        measure::snap::SnapKind::Path => "Path",
    })
}
/// Polylines with a dark halo underneath, legible on light, dark and busy drawings alike.
fn halo_lines(ui: &egui::Ui, lines: &[(Vec<Pos2>, bool)], color: Color32, width: f32) {
    for stroke in [Stroke::new(width + 2.5, Color32::from_black_alpha(190)), Stroke::new(width, color)] {
        for (pts, closed) in lines {
            ui.painter().add(if *closed { egui::Shape::closed_line(pts.clone(), stroke) } else { egui::Shape::line(pts.clone(), stroke) });
        }
    }
}
/// AutoSnap marker: square = endpoint, triangle = midpoint, X = intersection, hourglass = nearest.
fn snap_glyph(ui: &egui::Ui, c: Pos2, kind: measure::snap::SnapKind) {
    use measure::snap::SnapKind;
    let h = 6.0;
    let p = |dx: f32, dy: f32| c + egui::vec2(dx * h, dy * h);
    let lines: Vec<(Vec<Pos2>, bool)> = match kind {
        SnapKind::Endpoint => vec![(vec![p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)], true)],
        SnapKind::Midpoint => vec![(vec![p(0.0, -1.1), p(1.1, 0.9), p(-1.1, 0.9)], true)],
        SnapKind::Intersection => vec![(vec![p(-1.0, -1.0), p(1.0, 1.0)], false), (vec![p(1.0, -1.0), p(-1.0, 1.0)], false)],
        SnapKind::Path => vec![(vec![p(-1.0, -1.0), p(1.0, -1.0), p(-1.0, 1.0), p(1.0, 1.0)], true)],
    };
    halo_lines(ui, &lines, Color32::from_rgb(255, 212, 0), 2.0);
}
fn chip(ui: &egui::Ui, anchor: Pos2, align: egui::Align2, text: &str, bold: bool, fg: Color32, bg: Color32) {
    let font = if bold { crate::theme::semibold(12.0) } else { crate::theme::regular(11.5) };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let pad = egui::vec2(6.0, 3.0);
    let rect = align.anchor_size(anchor, galley.size() + pad * 2.0);
    ui.painter().rect_filled(rect, 3.0, bg);
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, fg.gamma_multiply(0.45)), egui::StrokeKind::Inside);
    ui.painter().galley(rect.min + pad, galley, fg);
}
fn finish(state: &mut MeasureView, pending: &mut Option<Edit>, doc: &Document, page: usize, tool: Tool, author: &str) {
    if tool == Tool::Calibrate {
        if let [a, b] = state.points.as_slice() {
            let a = doc.measurement_to_view(page, *a);
            let b = doc.measurement_to_view(page, *b);
            if let (Ok(a), Ok(b)) = (a, b) {
                let measured = measure::distance(a, b);
                if measured.is_finite() && measured > 1e-9 {
                    state.calibration = Some(Calibration { measured, text: String::new(), unit: state.unit.clone(), error: None });
                } else {
                    state.error = Some(tl!("Pick two different points.").into());
                }
            }
        }
        state.cancel();
        return;
    }
    let minimum = if tool == Tool::Area { 3 } else { 2 };
    if state.points.len() < minimum {
        state.error = Some(crate::i18n::fmt(tl!("This measurement needs at least {n} points."), &[("n", &minimum.to_string())]));
        return;
    }
    if let Some(scale) = state.scale.clone() {
        *pending = Some(Edit::AddMeasurement(NewMeasurement {
            page,
            kind: tool.kind(),
            points: state.points.clone(),
            scale,
            style: Style { color: [0.0, 0.47, 0.84], ..Style::default() },
            label: state.label.clone(),
            author: author.into(),
        }));
        state.cancel();
    }
}
pub(crate) fn panel(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    egui::ScrollArea::vertical().id_salt("measure-panel").show(ui, |ui| panel_body(app, ui, t));
}
fn panel_body(app: &mut PdfCraftApp, ui: &mut egui::Ui, _t: &Tokens) {
    crate::widgets::begin_dialog(ui);
    let Some((index, id)) = app.active_ids() else {
        ui.label(tl!("Open a PDF to measure it."));
        return;
    };
    let mut tool = None;
    ui.horizontal(|ui| {
        for (label, value) in [("Distance", Tool::Distance), ("Perimeter", Tool::Perimeter), ("Area", Tool::Area)] {
            if ui.selectable_label(app.quick_tool == QuickTool::Measure(value), tl!(label)).clicked() {
                tool = Some(value);
            }
        }
    });
    let Some(doc) = app.session.get(id) else { return };
    let units = app.units;
    let page = app.views[index].current;
    let state = &mut app.views[index].measure;
    if state.listing.as_ref().is_none_or(|(g, _)| *g != doc.edit_generation()) {
        state.listing = Some((doc.edit_generation(), doc.measurements()));
    }
    if state.seeded_page != Some(page) {
        // First seed: show the page side in the app's unit, so an unscaled page reads
        // "1 in = 1 in" (or "1 mm = 1 mm"), true size. Later pages keep the user's choice.
        if state.seeded_page.is_none() {
            state.paper_unit = units;
        }
        state.seeded_page = Some(page);
        let one = 1.0 / state.paper_unit.per_point();
        if let (Ok(a), Ok(b)) = (doc.measurement_to_user(page, [0.0, 0.0]), doc.measurement_to_user(page, [one, 0.0]))
            && let Ok(scale) = doc.measurement_scale_or(page, a, units.per_point(), units.label())
            && let Ok(reading) = measure::reading(Kind::Distance, &[a, b], &scale)
        {
            state.drawing_points = one;
            state.real_distance = reading.value;
            state.unit = scale.unit;
            state.precision = scale.precision;
        }
    }
    ui.label(tl!("Click vertices. Enter or double-click finishes a perimeter or area. Click the first point to close an area."));
    ui.label(tl!("Shift constrains to 45 degrees. Backspace removes a vertex. Escape cancels."));
    ui.separator();
    ui.label(tl!("Measurement information"));
    if let Some(reading) = &state.reading {
        ui.label(egui::RichText::new(&reading.label).strong());
        ui.label(format!("X: {:.3}   Y: {:.3}", reading.delta_x, reading.delta_y));
        ui.label(format!("{}: {:.2}°", tl!("Angle"), reading.angle));
        if let Some(scale) = &state.scale {
            ui.label(&scale.ratio);
        }
    } else {
        ui.label(tl!("Click the first point to begin."));
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Label"));
        ui.add(crate::widgets::line(&mut state.label).desired_width(130.0).char_limit(128));
    });
    ui.separator();
    ui.add(widgets::checkbox(&mut state.snap_enabled, tl!("Snap to drawing")));
    if state.snap_enabled {
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.endpoints, tl!("Endpoints")));
            ui.add(widgets::checkbox(&mut state.snaps.midpoints, tl!("Midpoints")));
        });
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut state.snaps.intersections, tl!("Intersections")));
            ui.add(widgets::checkbox(&mut state.snaps.paths, tl!("Paths")));
        });
        ui.add(egui::Slider::new(&mut state.sensitivity, 1.0..=20.0).text(tl!("Sensitivity")));
    }
    ui.separator();
    ui.label(tl!("Drawing scale"));
    // "1 mm on the drawing = 100 cm in real life".
    // Edits to the scale fields are stored in the page when the field is released, so nobody
    // has to know to press Apply (the readings use the stored scale).
    let (mut changed, mut busy, mut picked) = (false, false, false);
    let mut track = |r: egui::Response| {
        changed |= r.changed();
        busy |= r.dragged() || r.has_focus();
    };
    ui.horizontal(|ui| {
        let k = state.paper_unit.per_point();
        let mut paper = state.drawing_points * k;
        let r = ui.add(egui::DragValue::new(&mut paper).range(0.000001..=1e9).speed(0.1).max_decimals(6));
        if r.changed() && paper.is_finite() && paper > 0.0 {
            state.drawing_points = paper / k;
        }
        track(r);
        egui::ComboBox::from_id_salt("scale-paper-unit").width(54.0).selected_text(state.paper_unit.label()).show_ui(ui, |ui| {
            for u in Unit::ALL {
                if ui.selectable_label(state.paper_unit == u, u.label()).clicked() {
                    // Keep the length on the page; only the unit it is shown in changes.
                    state.paper_unit = u;
                }
            }
        });
        ui.label("=");
        track(ui.add(egui::DragValue::new(&mut state.real_distance).range(0.000001..=1e12).speed(0.1).max_decimals(6)));
        track(ui.add(crate::widgets::line(&mut state.unit).desired_width(44.0).char_limit(12)));
        ui.menu_button("▾", |ui| {
            for (name, _) in REAL_UNITS {
                if ui.button(name).clicked() {
                    state.unit = name.into();
                    picked = true;
                    ui.close();
                }
            }
        });
    });
    if let Some(ratio) = ratio_text(state.drawing_points, state.real_distance, &state.unit) {
        ui.label(egui::RichText::new(ratio).weak());
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Decimals"));
        track(ui.add(egui::DragValue::new(&mut state.precision).range(0..=6)));
    });
    if ui.button(tl!("Calibrate from two points")).clicked() {
        tool = Some(Tool::Calibrate);
    }
    track(ui.add(widgets::checkbox(&mut state.whole_page, tl!("Entire page"))));
    if !state.whole_page {
        ui.label(tl!("Viewport rectangle in page points (x, y, width, height)"));
        for (i, name) in ["X", "Y", "Width", "Height"].iter().enumerate() {
            if let Some(v) = state.rect.get_mut(i) {
                track(ui.add(egui::DragValue::new(v).range(0.0..=1e9).prefix(format!("{}: ", tl!(name)))));
            }
        }
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Viewport"));
        track(ui.add(crate::widgets::line(&mut state.name).desired_width(130.0).char_limit(128)));
    });
    state.scale_dirty |= changed || picked;
    let apply = ui.add_enabled(doc.allows_annotation(), egui::Button::new(tl!("Apply scale"))).clicked()
        || (state.scale_dirty && !busy && doc.allows_annotation());
    if let Some(error) = &state.error {
        ui.colored_label(Color32::from_rgb(190, 50, 50), error);
    }
    ui.separator();
    let mut select = None;
    match state.listing.as_ref().map(|(_, l)| l) {
        None => {}
        Some(Ok(listing)) => {
            ui.label(format!("{}: {}", tl!("Saved measurements"), listing.measurements.len()));
            if !listing.unsupported.is_empty() {
                let reasons: Vec<String> =
                    listing.unsupported.iter().map(|u| format!("{} {}: {}", tl!("Page"), u.page.saturating_add(1), u.reason)).collect();
                ui.colored_label(Color32::from_rgb(190, 120, 30), format!("{}: {}", tl!("Unsupported measurements"), listing.unsupported.len()))
                    .on_hover_text(reasons.join("\n"));
            }
            egui::ScrollArea::vertical().id_salt("measurement-list").max_height(140.0).show(ui, |ui| {
                for m in &listing.measurements {
                    if ui.button(format!("{} {}: {} {}", tl!("Page"), m.page.saturating_add(1), m.reading.label, m.label)).clicked() {
                        select = Some((m.page, m.index, m.reading.clone(), m.scale.clone()));
                    }
                }
            });
        }
        Some(Err(e)) => {
            ui.colored_label(Color32::from_rgb(190, 50, 50), e);
        }
    }
    let export = ui.button(tl!("Export measurements as CSV")).clicked();
    if apply {
        state.scale_dirty = false;
        match scale_edit(doc, page, state) {
            Ok(edit) => app.views[index].pending_edit = Some(edit),
            Err(e) => state.error = Some(e),
        }
    }
    if let Some((page, annotation, reading, scale)) = select {
        app.views[index].current = page;
        app.views[index].comments.selected = Some((page, annotation));
        app.views[index].measure.reading = Some(reading);
        app.views[index].measure.scale = Some(scale);
        app.quick_tool = QuickTool::Select;
    }
    if let Some(tool) = tool {
        app.views[index].measure.cancel();
        app.quick_tool = QuickTool::Measure(tool);
    }
    if export {
        export_csv(app);
    }
}
/// The edit that stores the scale in `state` as a viewport on `page` (the whole page or the rectangle).
fn scale_edit(doc: &Document, page: usize, state: &MeasureView) -> Result<Edit, String> {
    let info = doc.info.pages.get(page).ok_or_else(|| "no such page".to_string())?;
    let (a, b) = (doc.measurement_to_user(page, [0.0, 0.0])?, doc.measurement_to_user(page, [1.0, 0.0])?);
    let scale = Scale::calibrate(a, b, state.real_distance / state.drawing_points, &state.unit, state.precision).map_err(|e| e.to_string())?;
    let [x, y, w, h] = if state.whole_page { [0.0, 0.0, f64::from(info.width), f64::from(info.height)] } else { state.rect };
    let a = info.view_to_user(x as f32, y as f32);
    let b = info.view_to_user((x + w) as f32, (y + h) as f32);
    Ok(Edit::SetMeasurementScale {
        page,
        bbox: [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1])), f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))],
        name: state.name.clone(),
        scale,
    })
}
/// "Ratio 1 : 100" when both sides are lengths with a known size; `None` for custom units.
fn ratio_text(drawing_points: f64, real: f64, real_unit: &str) -> Option<String> {
    let per_mm = REAL_UNITS.iter().find(|(n, _)| n.eq_ignore_ascii_case(real_unit.trim()))?.1;
    let paper_mm = drawing_points * 25.4 / 72.0;
    let ratio = real * per_mm / paper_mm;
    (ratio.is_finite() && ratio > 0.0)
        .then(|| if ratio >= 1.0 { format!("{} 1 : {}", tl!("Ratio"), trim(ratio)) } else { format!("{} {} : 1", tl!("Ratio"), trim(1.0 / ratio)) })
}
fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
/// Asks for the real length of the two clicked points, then stores the resulting scale.
pub(crate) fn calibrate_dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((index, id)) = app.active_ids() else { return };
    let Some(state) = app.views.get_mut(index).map(|v| &mut v.measure) else { return };
    let Some(cal) = state.calibration.as_mut() else { return };
    let (mut ok, mut cancel) = (false, false);
    let paper = state.paper_unit;
    let modal = widgets::show_modal(ctx, "measure-calibrate", |ui| {
        ui.set_width(340.0);
        ui.label(egui::RichText::new(tl!("Calibrate scale")).font(crate::theme::semibold(16.0)));
        ui.add_space(6.0);
        ui.label(format!("{}: {:.2} {}", tl!("Measured on the drawing"), cal.measured * paper.per_point(), paper.label()));
        ui.label(tl!("What is the real length between these two points?"));
        ui.horizontal(|ui| {
            let edit = ui.add(widgets::line(&mut cal.text).desired_width(110.0).char_limit(24).hint_text(tl!("Length")));
            if !edit.has_focus() && cal.text.is_empty() {
                edit.request_focus();
            }
            ui.add(widgets::line(&mut cal.unit).desired_width(48.0).char_limit(12));
            ui.menu_button("▾", |ui| {
                for (name, _) in REAL_UNITS {
                    if ui.button(name).clicked() {
                        cal.unit = name.into();
                        ui.close();
                    }
                }
            });
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                ok = true;
            }
        });
        if let Some(e) = &cal.error {
            ui.colored_label(Color32::from_rgb(190, 50, 50), e);
        }
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ok |= widgets::pill_button(ui, tl!("Set scale"), true).clicked();
            cancel |= widgets::pill_button(ui, tl!("Cancel"), false).clicked();
        });
    });
    cancel |= modal.should_close();
    if cancel {
        state.calibration = None;
        return;
    }
    if !ok {
        return;
    }
    let value = cal.text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0);
    let Some(value) = value else {
        cal.error = Some(tl!("Enter a length greater than zero.").into());
        return;
    };
    state.drawing_points = cal.measured;
    state.real_distance = value;
    state.unit = cal.unit.trim().to_string();
    state.calibration = None;
    let Some(doc) = app.session.get(id) else { return };
    let page = app.views[index].current;
    let state = &mut app.views[index].measure;
    match scale_edit(doc, page, state) {
        Ok(edit) => {
            app.views[index].pending_edit = Some(edit);
            app.quick_tool = QuickTool::Measure(Tool::Distance);
        }
        Err(e) => state.error = Some(e),
    }
}
/// Opening Measure picks Distance, so a click on the page measures straight away. A tool the
/// user already chose (Perimeter, Area) stays.
pub(crate) fn default_tool(app: &mut PdfCraftApp) {
    if !matches!(app.quick_tool, QuickTool::Measure(_)) {
        app.quick_tool = QuickTool::Measure(Tool::Distance);
    }
}
pub(crate) fn command(app: &mut PdfCraftApp, id: &str) {
    app.left = LeftPanel::Tool("measure");
    app.left_open = true;
    default_tool(app);
    if let Some((index, _)) = app.active_ids()
        && let Some(tool) = id.strip_prefix("measure.").and_then(Tool::from_name)
    {
        app.views[index].measure.cancel();
        app.quick_tool = QuickTool::Measure(tool);
    }
    if id == "measure.export" {
        export_csv(app);
    }
}
fn export_csv(app: &mut PdfCraftApp) {
    let Some((_, id)) = app.active_ids() else { return };
    let result = app.session.get(id).ok_or_else(|| "no such document".to_string()).and_then(|d| d.measurements());
    match result {
        Err(e) => app.notify(&e),
        Ok(listing) => {
            let csv = measure::csv(&listing.measurements);
            if !listing.unsupported.is_empty() {
                app.notify(crate::i18n::fmt(
                    tl!("{n} measurements use an unsupported format and are left out."),
                    &[("n", &listing.unsupported.len().to_string())],
                ));
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let write = move |app: &mut PdfCraftApp, path: std::path::PathBuf| match crate::editing::write_atomically(
                    &path.to_string_lossy(),
                    csv.as_bytes(),
                ) {
                    Ok(()) => app.notify(tl!("Saved measurements.")),
                    Err(e) => app.notify(e.to_string()),
                };
                match app.export_dir_override.as_ref().map(|d| std::path::PathBuf::from(d).join("measurements.csv")) {
                    Some(path) => write(app, path),
                    None => {
                        let dialog = rfd::AsyncFileDialog::new().set_file_name("measurements.csv").add_filter("CSV", &["csv"]);
                        app.ask_one(crate::pickers::Ask::Save(dialog), None, write);
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            if let Err(e) = crate::editing::download("measurements.csv", csv.as_bytes()) {
                app.notify(&e);
            }
        }
    }
}
