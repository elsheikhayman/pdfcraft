//! Outline view (Illustrator's Ctrl+Y): the page's vector paths as hairlines, so linework hidden
//! under fills, hatches or images can be seen and measured. Uses the same path extraction as
//! measurement snapping, so what is drawn is what snaps.
use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Rect, Stroke};
use pdfcraft_engine::Document;
use pdfcraft_render::PageInfo;

use crate::canvas::PageXform;
use crate::theme::Tokens;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outline {
    #[default]
    Off,
    /// Paths only, on a blank page.
    Paths,
    /// The rendered page faded, with the paths drawn over it.
    Overlay,
}

impl Outline {
    /// Run again to leave: the same command turns its own mode off, or switches from the other.
    pub fn toggled(self, to: Outline) -> Outline {
        if self == to { Outline::Off } else { to }
    }
}

/// Segments of one page as normalised page coordinates (0..1 in view space, unrotated).
pub struct Lines {
    segments: Vec<[[f32; 2]; 2]>,
    truncated: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct OutlineCache {
    pages: HashMap<usize, (u64, Arc<Lines>)>,
}

impl OutlineCache {
    fn lines(&mut self, doc: &Document, page: usize, info: &PageInfo) -> Arc<Lines> {
        if let Some((generation, lines)) = self.pages.get(&page)
            && *generation == doc.edit_generation()
        {
            return lines.clone();
        }
        let (pw, ph) = (info.width.max(1.0), info.height.max(1.0));
        let lines = match doc.measurement_paths(page) {
            Ok(geometry) => {
                let norm = |p: [f64; 2]| {
                    let v = info.user_to_view(p[0] as f32, p[1] as f32);
                    [v[0] / pw, v[1] / ph]
                };
                let segments =
                    geometry.segments.iter().map(|[a, b]| [norm(*a), norm(*b)]).filter(|s| s.iter().flatten().all(|c| c.is_finite())).collect();
                Lines { segments, truncated: geometry.truncated, error: None }
            }
            Err(e) => Lines { segments: Vec::new(), truncated: false, error: Some(e) },
        };
        let lines = Arc::new(lines);
        // Keep the cache to pages near the one being looked at: a long document must not hold
        // every page's lines.
        if self.pages.len() >= 8 {
            self.pages.clear();
        }
        self.pages.insert(page, (doc.edit_generation(), lines.clone()));
        lines
    }
}

/// Fade the rendered page under an overlay.
pub fn fade(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_white_alpha(205));
}

/// Draw the page's paths. `visible` is the part of the screen worth drawing.
pub fn paint(painter: &egui::Painter, xf: &PageXform, page: usize, doc: &Document, info: &PageInfo, cache: &mut OutlineCache, visible: Rect) {
    let lines = cache.lines(doc, page, info);
    let tokens = Tokens::get(painter.ctx());
    let ppp = painter.ctx().pixels_per_point().max(1.0);
    let clip = xf.rect.intersect(visible);
    // A segment shorter than a third of a device pixel adds nothing to a hairline.
    let min_len = 0.35 / ppp;
    let mut segments: Vec<[egui::Pos2; 2]> = Vec::with_capacity(lines.segments.len().min(200_000));
    for [a, b] in &lines.segments {
        let (a, b) = (xf.norm_to_screen(a[0], a[1]), xf.norm_to_screen(b[0], b[1]));
        let bounds = Rect::from_two_pos(a, b);
        if !bounds.intersects(clip) || (bounds.width() < min_len && bounds.height() < min_len) {
            continue;
        }
        segments.push([a, b]);
    }
    if !segments.is_empty() {
        let stroke = Stroke::new(1.0, Color32::from_gray(25));
        painter.extend(segments.into_iter().map(|s| egui::Shape::line_segment(s, stroke)));
    }
    if let Some(error) = &lines.error {
        painter.text(xf.rect.center(), egui::Align2::CENTER_CENTER, error, crate::theme::regular(12.0), tokens.text_muted);
    } else if lines.truncated {
        let note = tl!("This drawing exceeds the snapping limit; only the first paths are used.");
        painter.text(
            xf.rect.left_top() + egui::vec2(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            note,
            crate::theme::regular(11.5),
            Color32::from_rgb(190, 120, 30),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Outline;

    #[test]
    fn a_command_turns_its_own_mode_off_and_switches_from_the_other() {
        assert_eq!(Outline::Off.toggled(Outline::Paths), Outline::Paths);
        assert_eq!(Outline::Paths.toggled(Outline::Paths), Outline::Off);
        assert_eq!(Outline::Paths.toggled(Outline::Overlay), Outline::Overlay);
        assert_eq!(Outline::Overlay.toggled(Outline::Overlay), Outline::Off);
    }
}
