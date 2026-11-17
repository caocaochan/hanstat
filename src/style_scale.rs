//! How writing-style ratings are shown. The model rates on a scale of 1
//! (plain, spoken) to 9 (near classical Chinese), and ratings are kept and
//! counted on it ([`hanstat::style::target_offset`]), but books use only a
//! narrow band of it, and which band depends on the books. So a rating is
//! shown relative to the rated books of the Library: on a logistic curve
//! centred on their median (50%), steep enough that their 5th and 95th
//! percentiles show as 5% and 95%. Whatever the spread, the books fill the
//! range; a book outside it comes close to 0% or 100% without reaching it.

/// A logistic curve fitted to a set of ratings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StyleScale {
    center: f32,
    /// Model points per unit of log-odds.
    spread: f32,
    /// How many ratings it was fitted to.
    pub(crate) books: usize,
}

/// Log-odds of 95%: the 95th percentile sits this many spreads above the median.
fn logit_95() -> f32 {
    19f32.ln()
}

/// Ratings that differ by less than this are one rating: the curve is never
/// steeper, so equal ratings don't divide by zero.
const MIN_SPREAD: f32 = 1e-3;

impl StyleScale {
    /// Fitted to `scores` (any order). With none, every rating is 50%.
    pub(crate) fn fit(scores: impl IntoIterator<Item = f32>) -> StyleScale {
        let mut v: Vec<f32> = scores.into_iter().filter(|x| x.is_finite()).collect();
        v.sort_by(f32::total_cmp);
        let n = v.len();
        if n == 0 {
            return StyleScale {
                center: 0.0,
                spread: f32::INFINITY,
                books: 0,
            };
        }
        // Linear interpolation between order statistics, so two books are enough.
        let q = |p: f32| {
            let x = p * (n - 1) as f32;
            let (i, f) = (x.floor() as usize, x.fract());
            let next = v[(i + 1).min(n - 1)];
            v[i] + (next - v[i]) * f
        };
        let spread = ((q(0.95) - q(0.05)) / (2.0 * logit_95())).max(MIN_SPREAD);
        StyleScale {
            center: q(0.5),
            spread,
            books: n,
        }
    }

    /// `score` as a percentage of the scale, 0–100.
    pub(crate) fn share(&self, score: f32) -> f32 {
        100.0 / (1.0 + (-(score - self.center) / self.spread).exp())
    }

    /// "47.3%".
    pub(crate) fn pct(&self, score: f32) -> String {
        format!("{:.1}%", self.share(score))
    }

    /// What a rating's hover says: the model's own number, which is kept and
    /// counted, and what the percentage is measured against.
    pub(crate) fn hover(&self, score: f32) -> String {
        format!(
            "The model's rating: {score:.2} on its scale of 1 (plain) to 9 (near classical Chinese); typical {:.2}. \
             It counts toward the difficulty with the weight set in ⚙.\n\
             {} against the {} rated books of your Library: 50% is their median ({:.2}).",
            hanstat::style::TYPICAL,
            self.pct(score),
            self.books,
            self.center,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    #[test]
    fn any_band_of_ratings_fills_the_range() {
        // Narrow and wide libraries, anywhere on the model's scale.
        for (lo, hi) in [(5.0f32, 6.6f32), (4.0, 9.0), (2.0, 6.0)] {
            let scores: Vec<f32> = (0..=100)
                .map(|i| lo + (hi - lo) * i as f32 / 100.0)
                .collect();
            let s = StyleScale::fit(scores.iter().copied());
            assert!(close(s.share((lo + hi) / 2.0), 50.0), "{lo}–{hi}: median");
            assert!(close(s.share(lo + 0.05 * (hi - lo)), 5.0), "{lo}–{hi}: p05");
            assert!(
                close(s.share(lo + 0.95 * (hi - lo)), 95.0),
                "{lo}–{hi}: p95"
            );
            assert!(s.share(lo) < 5.0 && s.share(lo) > 0.0);
            assert!(s.share(hi) > 95.0 && s.share(hi) < 100.0);
        }
    }

    #[test]
    fn order_and_odd_inputs() {
        // Order doesn't matter; NaN is ignored.
        let a = StyleScale::fit([6.2, 5.1, 5.9, f32::NAN, 5.5]);
        let b = StyleScale::fit([5.1, 5.5, 5.9, 6.2]);
        assert_eq!(a, b);
        assert!(a.share(5.1) < a.share(5.5) && a.share(5.9) < a.share(6.2));
        // Two books: one well below 50%, one well above.
        let two = StyleScale::fit([5.0, 6.0]);
        assert!(close(two.share(5.5), 50.0) && two.share(5.0) < 10.0 && two.share(6.0) > 90.0);
        // One book, or equal ratings: 50%, no division by zero.
        assert!(close(StyleScale::fit([5.7]).share(5.7), 50.0));
        assert!(close(StyleScale::fit([5.7, 5.7, 5.7]).share(5.7), 50.0));
        // None: everything at 50%.
        let none = StyleScale::fit([]);
        assert_eq!(none.books, 0);
        assert!(close(none.share(3.0), 50.0));
    }
}
