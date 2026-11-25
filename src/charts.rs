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
    canvas_sensing(ui, height, egui::Sense::hover())
}

fn canvas_sensing(ui: &mut egui::Ui, height: f32, sense: egui::Sense) -> Canvas {
    let w = ui.available_width().max(200.0);
    let (response, painter) = ui.allocate_painter(Vec2::new(w, height), sense);
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
        let gold = crate::palette::warn();
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

/// A day of [`calendar`]: its shade (0: nothing; 1..=4 darker), a ring
/// (the goal met) and its hover text.
pub struct CalDay {
    pub level: u8,
    pub ring: bool,
    pub hover: String,
}

/// The days a [`calendar`] shows and how.
pub struct Calendar<'a> {
    /// The first day's number and its weekday (0: Monday), and how many.
    pub first: i32,
    pub weekday: u32,
    pub n: i32,
    /// Month starts: day numbers and names.
    pub months: &'a [(i32, &'a str)],
    /// Empty, then light to dark.
    pub shades: [Color32; 5],
    pub ring: Color32,
    /// Day numbers `a..b` in focus: the other days are dimmed.
    pub focus: Option<(i32, i32)>,
}

/// A year as weeks across (Monday on top), one square a day, in one painter
/// pass. `day(i)`: the `i`-th day, `None` with nothing in it.
pub fn calendar(ui: &mut egui::Ui, cal: Calendar<'_>, day: &dyn Fn(i32) -> Option<CalDay>) {
    let Calendar {
        first,
        weekday,
        n,
        months,
        shades,
        ring,
        focus,
    } = cal;
    const GAP: f32 = 3.0;
    const LABEL_W: f32 = 30.0;
    const HEAD: f32 = 16.0;
    let weeks = ((weekday as i32 + n + 6) / 7).max(1);
    let avail = ui.available_width().max(200.0);
    let cell = ((avail - LABEL_W) / weeks as f32 - GAP).clamp(6.0, 15.0);
    let w = LABEL_W + weeks as f32 * (cell + GAP);
    let h = HEAD + 7.0 * (cell + GAP);
    let (response, painter) = ui.allocate_painter(Vec2::new(w, h), egui::Sense::hover());
    let o = response.rect.min;
    let weak = ui.visuals().weak_text_color();
    let pos = |i: i32| {
        let k = i + weekday as i32;
        pos2(
            o.x + LABEL_W + (k / 7) as f32 * (cell + GAP),
            o.y + HEAD + (k % 7) as f32 * (cell + GAP),
        )
    };
    for (row, name) in [(0, "Mon"), (2, "Wed"), (4, "Fri")] {
        painter.text(
            pos2(
                o.x + LABEL_W - 6.0,
                o.y + HEAD + row as f32 * (cell + GAP) + cell / 2.0,
            ),
            Align2::RIGHT_CENTER,
            name,
            small(),
            weak,
        );
    }
    let mut label_end = f32::MIN;
    for &(d, name) in months {
        let x = pos(d - first).x;
        if x > label_end {
            let g = painter.layout_no_wrap(name.to_owned(), small(), weak);
            label_end = x + g.size().x + 4.0;
            painter.galley(pos2(x, o.y), g, weak);
        }
    }
    let hovered = response.hover_pos().and_then(|p| {
        let k = ((p.x - o.x - LABEL_W) / (cell + GAP)).floor() as i32 * 7
            + ((p.y - o.y - HEAD) / (cell + GAP)).floor() as i32;
        let i = k - weekday as i32;
        (p.x >= o.x + LABEL_W && p.y >= o.y + HEAD && (0..n).contains(&i)).then_some(i)
    });
    let mut hover_text = None;
    for i in 0..n {
        let d = day(i);
        let r = Rect::from_min_size(pos(i), Vec2::splat(cell));
        let level = d.as_ref().map_or(0, |d| d.level.min(4));
        let dim = focus.is_some_and(|(a, b)| first + i < a || first + i >= b);
        let fade = |c: Color32| if dim { c.gamma_multiply(0.3) } else { c };
        painter.rect_filled(r, 2.0, fade(shades[level as usize]));
        if let Some(d) = &d
            && d.ring
        {
            painter.rect_stroke(
                r.shrink(0.5),
                2.0,
                Stroke::new(1.5, fade(ring)),
                StrokeKind::Inside,
            );
        }
        if hovered == Some(i) {
            painter.rect_stroke(
                r.expand(1.0),
                2.0,
                Stroke::new(1.5, ui.visuals().text_color()),
                StrokeKind::Outside,
            );
            hover_text = Some(d.map_or_else(String::new, |d| d.hover));
        }
    }
    if let Some(t) = hover_text.filter(|t| !t.is_empty()) {
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(t);
        });
    }
}

/// A note on a [`running_total`] curve: `x` (in days) is where its dot sits
/// on the curve.
pub struct Note {
    pub x: f64,
    pub text: String,
    pub hover: String,
    /// A click on its label or dot is reported.
    pub link: bool,
}

/// What a [`running_total`] shows.
pub struct Running<'a> {
    /// Days `first..end` along the x axis.
    pub first: i32,
    pub end: i32,
    /// Amounts by day (sorted, inside `first..end`): each adds to the
    /// curve over its day.
    pub steps: &'a [(i32, f64)],
    /// Most important first: those with no room are left out.
    pub notes: &'a [Note],
    /// Labels under the x axis: day and text.
    pub ticks: &'a [(i32, String)],
    pub color: Color32,
    pub height: f32,
}

/// A running total over days, rising with each day's amount, with notes
/// on leader lines placed clear of each other and of the curve. `hover(day,
/// total)`: the text for a day. Returns the linked note clicked, if any.
pub fn running_total(
    ui: &mut egui::Ui,
    r: Running<'_>,
    hover: &dyn Fn(i32, f64) -> String,
) -> Option<usize> {
    let c = canvas_sensing(ui, r.height, egui::Sense::click());
    let total: f64 = r.steps.iter().map(|s| s.1).sum();
    let (top, ticks) = nice_ticks(total);
    c.y_axis(0.0, top, &ticks, &compact);
    c.baseline();
    let span = (r.end - r.first).max(1) as f64;
    let px = |d: f64| c.plot.left() + ((d - r.first as f64) / span) as f32 * c.plot.width();
    let day_at = |x: f32| r.first as f64 + ((x - c.plot.left()) / c.plot.width()) as f64 * span;
    for (d, s) in r.ticks {
        let x = px(*d as f64);
        c.painter.vline(
            x,
            c.plot.bottom()..=c.plot.bottom() + 3.0,
            Stroke::new(1.0, c.weak),
        );
        c.x_label(x, s);
    }
    // The curve's corners, in days: flat between days read, rising over
    // each day read.
    let mut pts: Vec<(f64, f64)> = vec![(r.first as f64, 0.0)];
    let mut sum = 0.0;
    for &(d, v) in r.steps {
        if v <= 0.0 {
            continue;
        }
        pts.push((d as f64, sum));
        sum += v;
        pts.push((d as f64 + 1.0, sum));
    }
    pts.push((r.end as f64, sum));
    // The total at day `d`, between corners.
    let at = |d: f64| -> f64 {
        let i = pts.partition_point(|p| p.0 <= d);
        if i == 0 {
            return 0.0;
        }
        if i >= pts.len() {
            return sum;
        }
        let (a, b) = (pts[i - 1], pts[i]);
        if b.0 > a.0 {
            a.1 + (b.1 - a.1) * (d - a.0) / (b.0 - a.0)
        } else {
            b.1
        }
    };
    let screen: Vec<Pos2> = pts
        .iter()
        .map(|&(d, v)| pos2(px(d), c.y(v, 0.0, top)))
        .collect();
    // A light fill under the curve, one quad per stretch.
    let mut mesh = egui::Mesh::default();
    let fill = r.color.gamma_multiply(0.12);
    for w in screen.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b.x - a.x < 0.01 {
            continue;
        }
        let i = mesh.vertices.len() as u32;
        for p in [a, b, pos2(b.x, c.plot.bottom()), pos2(a.x, c.plot.bottom())] {
            mesh.colored_vertex(p, fill);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    c.painter.add(mesh);
    c.painter.line(screen.clone(), Stroke::new(2.5, r.color));

    // The curve rises left to right, so a label is clear of it when its
    // bottom is above the curve at its right edge, or its top below the
    // curve at its left edge.
    let curve_y = |x: f32| c.y(at(day_at(x)), 0.0, top);
    let font = FontId::proportional(12.0);
    let area = c.plot.shrink2(Vec2::new(2.0, 2.0));
    let mut taken: Vec<Rect> = Vec::new();
    let mut placed: Vec<(usize, Pos2, Option<Rect>)> = Vec::new();
    for (i, n) in r.notes.iter().enumerate() {
        let x = n.x.clamp(r.first as f64, r.end as f64);
        let dot = pos2(px(x), c.y(at(x), 0.0, top));
        let g = c
            .painter
            .layout_no_wrap(n.text.clone(), font.clone(), c.text);
        let size = g.size() + Vec2::new(4.0, 2.0);
        let mut best = None;
        // Alternate the preferred side so neighbours spread out.
        let sides = if i % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        };
        'search: for dist in [16.0, 30.0, 46.0, 64.0, 86.0, 112.0, 142.0, 176.0] {
            for above in sides {
                for slide in [0.3, 0.0, 0.8] {
                    let rect = if above {
                        let right = dot.x - dist * slide + size.x * 0.35;
                        Rect::from_min_size(pos2(right - size.x, dot.y - dist - size.y), size)
                    } else {
                        let left = dot.x + dist * slide - size.x * 0.35;
                        Rect::from_min_size(pos2(left, dot.y + dist), size)
                    };
                    let dx = (area.left() - rect.left()).max(0.0)
                        + (area.right() - rect.right()).min(0.0);
                    let rect = rect.translate(Vec2::new(dx, 0.0));
                    if !area.contains_rect(rect) {
                        continue;
                    }
                    let clear = if above {
                        rect.bottom() < curve_y(rect.right()) - 4.0
                    } else {
                        rect.top() > curve_y(rect.left()) + 4.0
                    };
                    let free = !taken.iter().any(|t| t.intersects(rect.expand(3.0)))
                        && !placed.iter().any(|(_, p, _)| rect.expand(4.0).contains(*p));
                    if clear && free && !rect.expand(4.0).contains(dot) {
                        best = Some(rect);
                        break 'search;
                    }
                }
            }
        }
        if let Some(rect) = best {
            taken.push(rect);
            let end = pos2(
                dot.x.clamp(rect.left(), rect.right()),
                dot.y.clamp(rect.top(), rect.bottom()),
            );
            c.painter
                .line_segment([dot, end], Stroke::new(1.0, c.weak.gamma_multiply(0.8)));
            c.painter.galley(rect.min + Vec2::new(2.0, 1.0), g, c.text);
        }
        placed.push((i, dot, best));
    }
    for (_, dot, _) in &placed {
        c.painter.circle(*dot, 3.5, r.color, Stroke::new(1.5, c.bg));
    }

    let pointer = c.response.hover_pos();
    let on_note = pointer.and_then(|m| {
        placed
            .iter()
            .find(|(_, dot, rect)| dot.distance(m) < 8.0 || rect.is_some_and(|r| r.contains(m)))
            .map(|(i, dot, rect)| (*i, *dot, *rect))
    });
    let mut clicked = None;
    let text = if let Some((i, dot, rect)) = on_note {
        c.painter
            .circle(dot, 5.5, r.color, Stroke::new(2.0, c.text));
        if r.notes[i].link {
            c.response
                .ctx
                .set_cursor_icon(egui::CursorIcon::PointingHand);
            if let Some(rect) = rect {
                c.painter.hline(
                    rect.x_range().shrink(2.0),
                    rect.bottom() - 1.0,
                    Stroke::new(1.0, c.text),
                );
            }
            if c.response.clicked() {
                clicked = Some(i);
            }
        }
        Some(r.notes[i].hover.clone())
    } else {
        pointer.filter(|m| c.plot.contains(*m)).map(|m| {
            let day = (day_at(m.x).floor() as i32).clamp(r.first, (r.end - 1).max(r.first));
            let x = px(day as f64 + 1.0);
            let v = at(day as f64 + 1.0);
            c.painter
                .vline(x, c.plot.y_range(), Stroke::new(1.0, c.weak));
            c.painter.circle(
                pos2(x, c.y(v, 0.0, top)),
                4.5,
                r.color,
                Stroke::new(1.5, c.text),
            );
            hover(day, v)
        })
    };
    c.hover(text);
    clicked
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
                let note = |x| Note {
                    x,
                    text: "《书》".into(),
                    hover: "n".into(),
                    link: x > 5.0,
                };
                for (first, end, steps, notes) in [
                    (5, 5, vec![], vec![]),
                    (5, 6, vec![(5, 100.0)], vec![note(5.5)]),
                    (
                        0,
                        400,
                        (0..400).step_by(3).map(|d| (d, 500.0)).collect(),
                        (0..40).map(|i| note(i as f64 * 10.0)).collect(),
                    ),
                ] {
                    let _ = running_total(
                        ui,
                        Running {
                            first,
                            end,
                            steps: &steps,
                            notes: &notes,
                            ticks: &[(first, "Jan".into())],
                            color: Color32::BLUE,
                            height: 200.0,
                        },
                        &|d, v| format!("{d} {v}"),
                    );
                }
                let shades = [Color32::GRAY; 5];
                for (wd, n) in [(0, 365), (6, 366), (3, 1)] {
                    let cal = Calendar {
                        first: 100,
                        weekday: wd,
                        n,
                        months: &[(100, "Jan")],
                        shades,
                        ring: Color32::RED,
                        focus: (wd == 6).then_some((105, 112)),
                    };
                    calendar(ui, cal, &|i| {
                        (i % 3 == 0).then(|| CalDay {
                            level: (i % 6) as u8,
                            ring: i % 2 == 0,
                            hover: "d".into(),
                        })
                    });
                }
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
