//! Small charts drawn with egui's painter, for the Library's Stats tab:
//! columns, a scatter plot and a coverage curve. Colours follow the theme;
//! hovering a mark shows what it is.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2};

pub struct Bar {
    pub value: f64,
    pub color: Color32,
    pub hover: String,
}

pub struct Point {
    pub x: f64,
    pub y: f64,
    pub color: Color32,
    pub hover: String,
}

/// A horizontal band behind a plot, from `lo` to `hi` on the y axis.
pub struct Band {
    pub lo: f64,
    pub hi: f64,
    pub color: Color32,
    pub label: &'static str,
}

const LEFT: f32 = 52.0;
const BOTTOM: f32 = 22.0;
const TOP: f32 = 8.0;
const RIGHT: f32 = 10.0;

fn small() -> FontId {
    FontId::proportional(11.0)
}

struct Canvas {
    painter: egui::Painter,
    response: egui::Response,
    plot: Rect,
    text: Color32,
    weak: Color32,
    grid: Stroke,
    bg: Color32,
}

fn canvas(ui: &mut egui::Ui, height: f32) -> Canvas {
    let w = ui.available_width().max(200.0);
    let (response, painter) = ui.allocate_painter(Vec2::new(w, height), egui::Sense::hover());
    let r = response.rect;
    let plot = Rect::from_min_max(
        pos2(r.left() + LEFT, r.top() + TOP),
        pos2(r.right() - RIGHT, r.bottom() - BOTTOM),
    );
    let v = ui.visuals();
    Canvas {
        painter,
        plot,
        text: v.text_color(),
        weak: v.weak_text_color(),
        grid: Stroke::new(
            1.0,
            v.widgets.noninteractive.bg_stroke.color.gamma_multiply(0.6),
        ),
        bg: v.panel_fill,
        response,
    }
}

impl Canvas {
    /// Horizontal grid lines and labels at `ticks` on a linear y axis `lo..hi`.
    fn y_axis(&self, lo: f64, hi: f64, ticks: &[f64], fmt: &dyn Fn(f64) -> String) {
        for &t in ticks {
            let y = self.y(t, lo, hi);
            self.painter.hline(self.plot.x_range(), y, self.grid);
            self.painter.text(
                pos2(self.plot.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                fmt(t),
                small(),
                self.weak,
            );
        }
    }

    fn y(&self, v: f64, lo: f64, hi: f64) -> f32 {
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        self.plot.bottom() - t * self.plot.height()
    }

    /// Centred under `x`, moved in where it would cross the canvas edge.
    /// Returns its width.
    fn x_label(&self, x: f32, s: &str) -> f32 {
        let g = self
            .painter
            .layout_no_wrap(s.to_owned(), small(), self.weak);
        let w = g.size().x;
        let bounds = self.response.rect;
        let left = (x - w / 2.0).clamp(bounds.left(), (bounds.right() - w).max(bounds.left()));
        self.painter
            .galley(pos2(left, self.plot.bottom() + 4.0), g, self.weak);
        w
    }

    fn baseline(&self) {
        self.painter.hline(
            self.plot.x_range(),
            self.plot.bottom(),
            Stroke::new(1.0, self.weak),
        );
    }

    fn hover(self, text: Option<String>) {
        if let Some(t) = text {
            self.response.on_hover_ui_at_pointer(|ui| {
                ui.label(t);
            });
        }
    }
}

/// Logarithmic x axis from `lo` to `hi` (both > 0).
struct LogX {
    lo: f64,
    hi: f64,
}

impl LogX {
    fn x(&self, v: f64, plot: Rect) -> f32 {
        let (a, b) = (self.lo.ln(), self.hi.ln());
        let t = if b > a {
            (v.max(self.lo).ln() - a) / (b - a)
        } else {
            0.5
        };
        plot.left() + t.clamp(0.0, 1.0) as f32 * plot.width()
    }

    fn inv(&self, x: f32, plot: Rect) -> f64 {
        let t = ((x - plot.left()) / plot.width()).clamp(0.0, 1.0) as f64;
        (self.lo.ln() + t * (self.hi.ln() - self.lo.ln())).exp()
    }

    /// Grid lines at 1, 10, 100…, labelled, and lighter ones at 2× and 5×.
    fn draw(&self, c: &Canvas) {
        let mut d = 10f64.powf(self.lo.log10().floor());
        while d <= self.hi * 1.0001 {
            for (m, major) in [(1.0, true), (2.0, false), (5.0, false)] {
                let v = d * m;
                if v < self.lo * 0.9999 || v > self.hi * 1.0001 {
                    continue;
                }
                let x = self.x(v, c.plot);
                let stroke = if major {
                    c.grid
                } else {
                    Stroke::new(1.0, c.grid.color.gamma_multiply(0.4))
                };
                c.painter.vline(x, c.plot.y_range(), stroke);
                if major || self.hi / self.lo < 1e3 {
                    c.x_label(x, &compact(v));
                }
            }
            d *= 10.0;
        }
    }
}

/// Columns, one per bar, on a linear axis from zero. `labels`, if given,
/// are written under the columns.
pub fn columns(
    ui: &mut egui::Ui,
    bars: &[Bar],
    labels: Option<&[String]>,
    height: f32,
    fmt: &dyn Fn(f64) -> String,
) {
    let c = canvas(ui, height);
    let max = bars.iter().map(|b| b.value).fold(0.0, f64::max);
    let (top, ticks) = nice_ticks(max);
    c.y_axis(0.0, top, &ticks, fmt);
    c.baseline();
    if bars.is_empty() {
        return;
    }
    let slot = c.plot.width() / bars.len() as f32;
    let gap = if slot > 6.0 {
        (slot * 0.18).min(10.0)
    } else {
        0.0
    };
    let pointer = c
        .response
        .hover_pos()
        .filter(|p| c.plot.expand(4.0).contains(*p));
    let hovered = pointer.map(|p| (((p.x - c.plot.left()) / slot) as usize).min(bars.len() - 1));
    let mut label_end = f32::MIN;
    for (i, b) in bars.iter().enumerate() {
        let x0 = c.plot.left() + i as f32 * slot + gap / 2.0;
        let r = Rect::from_min_max(
            pos2(x0, c.y(b.value, 0.0, top)),
            pos2(x0 + slot - gap, c.plot.bottom()),
        );
        let color = if hovered == Some(i) {
            b.color
        } else {
            b.color.gamma_multiply(0.85)
        };
        let radius = if slot > 8.0 { 2.0 } else { 0.0 };
        c.painter.rect_filled(
            r,
            egui::CornerRadius {
                nw: radius as u8,
                ne: radius as u8,
                sw: 0,
                se: 0,
            },
            color,
        );
        if hovered == Some(i) {
            c.painter.rect_stroke(
                r.expand(1.0),
                2.0,
                Stroke::new(1.5, c.text),
                StrokeKind::Outside,
            );
        }
        if let Some(l) = labels.and_then(|l| l.get(i)) {
            let mid = x0 + (slot - gap) / 2.0;
            let w = c
                .painter
                .layout_no_wrap(l.clone(), small(), c.weak)
                .size()
                .x;
            if mid - w / 2.0 > label_end + 4.0 {
                c.x_label(mid, l);
                label_end = mid + w / 2.0;
            }
        }
    }
    let text = hovered.map(|i| bars[i].hover.clone());
    c.hover(text);
}

/// Points on a log x axis and a linear y axis `y_lo..y_hi`, over `bands`.
pub fn scatter(
    ui: &mut egui::Ui,
    points: &[Point],
    bands: &[Band],
    (y_lo, y_hi): (f64, f64),
    height: f32,
) {
    let c = canvas(ui, height);
    for b in bands {
        let r = Rect::from_x_y_ranges(
            c.plot.x_range(),
            c.y(b.hi.min(y_hi), y_lo, y_hi)..=c.y(b.lo.max(y_lo), y_lo, y_hi),
        );
        if r.height() > 0.5 {
            c.painter.rect_filled(r, 0.0, b.color.gamma_multiply(0.10));
            if r.height() > 12.0 {
                c.painter.text(
                    pos2(c.plot.right() - 4.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    b.label,
                    small(),
                    b.color.gamma_multiply(0.8),
                );
            }
        }
    }
    let step = if y_hi - y_lo > 30.0 { 10.0 } else { 5.0 };
    let ticks: Vec<f64> = (0..)
        .map(|i| (y_lo / step).ceil() * step + i as f64 * step)
        .take_while(|&t| t <= y_hi + 1e-9)
        .collect();
    c.y_axis(y_lo, y_hi, &ticks, &|t| format!("{t:.0}%"));
    c.baseline();
    if points.is_empty() {
        return;
    }
    let lo = points.iter().map(|p| p.x).fold(f64::MAX, f64::min).max(1.0);
    let hi = points.iter().map(|p| p.x).fold(0.0, f64::max).max(lo * 1.5);
    let ax = LogX {
        lo: lo / 1.2,
        hi: hi * 1.2,
    };
    ax.draw(&c);
    let pos = |p: &Point| pos2(ax.x(p.x, c.plot), c.y(p.y, y_lo, y_hi));
    let pointer = c.response.hover_pos();
    let hovered = pointer.and_then(|m| {
        points
            .iter()
            .enumerate()
            .map(|(i, p)| (i, pos(p).distance(m)))
            .filter(|(_, d)| *d < 12.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    });
    for (i, p) in points.iter().enumerate() {
        if hovered != Some(i) {
            c.painter.circle(
                pos(p),
                4.5,
                p.color.gamma_multiply(0.85),
                Stroke::new(1.0, p.color),
            );
        }
    }
    if let Some(i) = hovered {
        let p = &points[i];
        c.painter
            .circle(pos(p), 6.5, p.color, Stroke::new(2.0, c.text));
    }
    let text = hovered.map(|i| points[i].hover.clone());
    c.hover(text);
}

/// A rising curve of (n, percent) on a log x axis, with dashed guides at
/// `marks` (percent, n) and an optional horizontal line `level` (percent, label).
pub fn coverage_curve(
    ui: &mut egui::Ui,
    pts: &[(usize, f64)],
    marks: &[(f64, usize)],
    level: Option<(f64, String)>,
    color: Color32,
    unit: &str,
    height: f32,
) {
    let c = canvas(ui, height);
    let ticks = [0.0, 20.0, 40.0, 60.0, 80.0, 100.0];
    c.y_axis(0.0, 100.0, &ticks, &|t| format!("{t:.0}%"));
    c.baseline();
    let Some(&(last, _)) = pts.last() else {
        return;
    };
    let ax = LogX {
        lo: 1.0,
        hi: (last as f64).max(10.0),
    };
    ax.draw(&c);
    let p = |n: f64, v: f64| pos2(ax.x(n, c.plot), c.y(v, 0.0, 100.0));
    for &(pct, n) in marks {
        let at = p(n as f64, pct);
        dashed(&c.painter, pos2(c.plot.left(), at.y), at, c.weak);
        dashed(&c.painter, at, pos2(at.x, c.plot.bottom()), c.weak);
        c.painter.circle_filled(at, 3.0, color);
    }
    if let Some((v, label)) = &level {
        let y = c.y(*v, 0.0, 100.0);
        let gold = Color32::from_rgb(220, 170, 60);
        c.painter.hline(c.plot.x_range(), y, Stroke::new(1.5, gold));
        c.painter.text(
            pos2(c.plot.left() + 6.0, y - 2.0),
            Align2::LEFT_BOTTOM,
            label,
            small(),
            gold,
        );
    }
    let line: Vec<Pos2> = pts.iter().map(|&(n, v)| p(n as f64, v)).collect();
    c.painter.line(line, Stroke::new(2.0, color));
    // The marks crowd together on a log axis, so their figures go in a
    // legend at the bottom right, under the curve's flat end, on a panel
    // the guide lines don't cross.
    if !marks.is_empty() {
        let galleys: Vec<_> = marks
            .iter()
            .map(|&(pct, n)| {
                c.painter.layout_no_wrap(
                    format!("{pct:.0}% of the text: {} {unit}", crate::fmt_int(n as u64)),
                    small(),
                    c.text,
                )
            })
            .collect();
        let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
        let line_h = galleys[0].size().y + 2.0;
        let pad = 5.0;
        let size = Vec2::new(w + 2.0 * pad, line_h * galleys.len() as f32 + 2.0 * pad);
        let panel = Rect::from_min_size(c.plot.right_bottom() - size - Vec2::new(4.0, 4.0), size);
        c.painter.rect_filled(panel, 3.0, c.bg);
        c.painter
            .rect_stroke(panel, 3.0, c.grid, StrokeKind::Inside);
        for (i, g) in galleys.into_iter().enumerate() {
            let x = panel.right() - pad - g.size().x;
            c.painter
                .galley(pos2(x, panel.top() + pad + i as f32 * line_h), g, c.text);
        }
    }

    let hover = c
        .response
        .hover_pos()
        .filter(|m| c.plot.contains(*m))
        .map(|m| {
            let n = ax.inv(m.x, c.plot);
            // The last sampled point at or before n.
            let i = pts.partition_point(|&(r, _)| (r as f64) <= n).max(1) - 1;
            let (r, v) = pts[i];
            let at = p(r as f64, v);
            c.painter
                .vline(at.x, c.plot.y_range(), Stroke::new(1.0, c.weak));
            c.painter.circle(at, 5.0, color, Stroke::new(1.5, c.text));
            format!(
                "The {} most frequent {unit} make up {v:.1}% of the text",
                crate::fmt_int(r as u64)
            )
        });
    c.hover(hover);
}

/// One bar split into segments (count, colour, label), with a legend below.
pub fn stacked_bar(ui: &mut egui::Ui, parts: &[(usize, Color32, String)]) {
    let total: usize = parts.iter().map(|p| p.0).sum();
    let w = ui.available_width().max(200.0);
    let (response, painter) = ui.allocate_painter(Vec2::new(w, 26.0), egui::Sense::hover());
    let r = response.rect;
    let mut x = r.left();
    let mut hovered = None;
    for (n, color, label) in parts {
        if *n == 0 || total == 0 {
            continue;
        }
        let seg = Rect::from_min_max(
            pos2(x, r.top()),
            pos2(x + r.width() * *n as f32 / total as f32, r.bottom()),
        );
        painter.rect_filled(seg.shrink2(Vec2::new(1.0, 0.0)), 3.0, *color);
        if seg.width() > 24.0 {
            painter.text(
                seg.center(),
                Align2::CENTER_CENTER,
                n.to_string(),
                FontId::proportional(13.0),
                Color32::BLACK,
            );
        }
        if response.hover_pos().is_some_and(|p| seg.contains(p)) {
            hovered = Some(format!("{label}: {n} of {total} books"));
            painter.rect_stroke(
                seg,
                3.0,
                Stroke::new(1.5, ui.visuals().text_color()),
                StrokeKind::Inside,
            );
        }
        x = seg.right();
    }
    if let Some(t) = hovered {
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(t);
        });
    }
    ui.horizontal_wrapped(|ui| {
        for (n, color, label) in parts {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, *color);
            ui.label(egui::RichText::new(format!("{label} {n}")).small());
            ui.add_space(6.0);
        }
    });
}

fn dashed(p: &egui::Painter, a: Pos2, b: Pos2, color: Color32) {
    p.extend(egui::Shape::dashed_line(
        &[a, b],
        Stroke::new(1.0, color),
        4.0,
        3.0,
    ));
}

/// A round top for a linear axis from zero, and ticks up to it.
fn nice_ticks(max: f64) -> (f64, Vec<f64>) {
    if max <= 0.0 {
        return (1.0, vec![0.0, 1.0]);
    }
    let raw = max / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let top = (max / step).ceil() * step;
    let n = (top / step).round() as usize;
    (top, (0..=n).map(|i| i as f64 * step).collect())
}

/// 950, 1.2k, 35k, 1.4M.
pub fn compact(v: f64) -> String {
    let a = v.abs();
    let (x, s) = if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "k")
    } else {
        return format!("{v:.0}");
    };
    if x.abs() >= 10.0 || (x - x.round()).abs() < 0.05 {
        format!("{x:.0}{s}")
    } else {
        format!("{x:.1}{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chart draws without panicking, empty or with one mark, and
    /// while hovered.
    #[test]
    fn draws_edge_cases() {
        let ctx = egui::Context::default();
        for hover in [None, Some(pos2(300.0, 100.0))] {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 1200.0))),
                ..Default::default()
            };
            if let Some(p) = hover {
                input.events.push(egui::Event::PointerMoved(p));
            }
            let mut out = ctx.run_ui(input, |ui| {
                let bar = |v| Bar {
                    value: v,
                    color: Color32::RED,
                    hover: "b".into(),
                };
                let pt = |x, y| Point {
                    x,
                    y,
                    color: Color32::RED,
                    hover: "p".into(),
                };
                for bars in [
                    vec![],
                    vec![bar(0.0)],
                    (0..500).map(|i| bar(i as f64)).collect(),
                ] {
                    columns(ui, &bars, None, 120.0, &compact);
                }
                columns(ui, &[bar(3.0)], Some(&["1".to_owned()]), 120.0, &compact);
                for pts in [
                    vec![],
                    vec![pt(1.0, 100.0)],
                    vec![pt(1000.0, 90.0), pt(5e5, 70.0)],
                ] {
                    scatter(ui, &pts, &[], (60.0, 100.0), 120.0);
                }
                coverage_curve(ui, &[], &[], None, Color32::BLUE, "words", 120.0);
                coverage_curve(
                    ui,
                    &[(1, 100.0)],
                    &[(98.0, 1)],
                    Some((50.0, "x".into())),
                    Color32::BLUE,
                    "words",
                    120.0,
                );
                stacked_bar(ui, &[(0, Color32::RED, "a".into())]);
                stacked_bar(
                    ui,
                    &[
                        (3, Color32::RED, "a".into()),
                        (1, Color32::GREEN, "b".into()),
                    ],
                );
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn ticks_and_compact() {
        assert_eq!(nice_ticks(950_000.0).0, 1_000_000.0);
        assert_eq!(nice_ticks(9.0).1, [0.0, 2.5, 5.0, 7.5, 10.0]);
        assert_eq!(compact(950.0), "950");
        assert_eq!(compact(1_234.0), "1.2k");
        assert_eq!(compact(35_000.0), "35k");
        assert_eq!(compact(2_000_000.0), "2M");
    }
}
