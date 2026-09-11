//! Animation clock, easing, and the small "effect" vocabulary the views use.
//!
//! All time is milliseconds since app start, taken from one clock in `app.rs`,
//! so frame dumps can drive the same code with synthetic timestamps.

use crate::theme::{Rgb, lerp};

pub const FPS_BUSY_MS: u64 = 33; // ~30fps while something moves
pub const FPS_IDLE_MS: u64 = 120; // slow poll when the screen is settled

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    OutCubic,
    OutQuint,
    InOutSine,
    OutBack,
}

pub fn ease(e: Ease, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match e {
        Ease::Linear => t,
        Ease::OutCubic => 1.0 - (1.0 - t).powi(3),
        Ease::OutQuint => 1.0 - (1.0 - t).powi(5),
        Ease::InOutSine => -((std::f32::consts::PI * t).cos() - 1.0) / 2.0,
        Ease::OutBack => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
        }
    }
}

/// A time window with an easing curve. `t(now, start)` is the eased progress.
#[derive(Clone, Copy)]
pub struct Tween {
    pub delay: u64,
    pub dur: u64,
    pub ease: Ease,
}

impl Tween {
    pub const fn new(dur: u64, ease: Ease) -> Self {
        Self {
            delay: 0,
            dur,
            ease,
        }
    }

    pub const fn delayed(delay: u64, dur: u64, ease: Ease) -> Self {
        Self { delay, dur, ease }
    }

    /// Raw 0..1 progress, no easing, unclamped before the start.
    pub fn raw(&self, now: u64, start: u64) -> f32 {
        if now < start + self.delay {
            return 0.0;
        }
        if self.dur == 0 {
            return 1.0;
        }
        ((now - start - self.delay) as f32 / self.dur as f32).clamp(0.0, 1.0)
    }

    pub fn t(&self, now: u64, start: u64) -> f32 {
        ease(self.ease, self.raw(now, start))
    }

    pub fn done(&self, now: u64, start: u64) -> bool {
        now >= start + self.delay + self.dur
    }
}

/// Smooth 0..1..0 breathing value.
pub fn breathe(now: u64, period: u64) -> f32 {
    let phase = (now % period) as f32 / period as f32;
    (phase * std::f32::consts::TAU).sin() * 0.5 + 0.5
}

/// Sawtooth 0..1, for shimmer bands that should keep travelling one way.
pub fn saw(now: u64, period: u64) -> f32 {
    (now % period) as f32 / period as f32
}

/// Gaussian falloff, used to light up cells near a moving band.
pub fn bell(x: f32, center: f32, width: f32) -> f32 {
    let d = (x - center) / width;
    (-d * d).exp()
}

/// Braille spinner, the terminal-native loading indicator.
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(now: u64) -> &'static str {
    SPIN[((now / 80) % SPIN.len() as u64) as usize]
}

/// A number that walks to its new value instead of jumping. Counters are the
/// one place a terminal can feel smooth, so tokens and cost use this.
#[derive(Clone, Copy, Default)]
pub struct Anim {
    from: f32,
    to: f32,
    at: u64,
    dur: u64,
}

impl Anim {
    pub const fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            at: 0,
            dur: 0,
        }
    }

    pub fn set(&mut self, target: f32, now: u64, dur: u64) {
        self.from = self.to;
        self.to = target;
        self.at = now;
        self.dur = dur.max(1);
    }

    pub fn get(&self, now: u64) -> f32 {
        if self.from == self.to || self.dur == 0 {
            return self.to;
        }
        let t = ease(
            Ease::OutCubic,
            (now.saturating_sub(self.at)) as f32 / self.dur as f32,
        );
        self.from + (self.to - self.from) * t
    }

    pub fn done(&self, now: u64) -> bool {
        now >= self.at + self.dur
    }
}

/// A short horizontal wobble, used to shake a card that failed.
pub fn shake(now: u64, at: u64, dur: u64) -> i8 {
    if now < at || now >= at + dur {
        return 0;
    }
    let t = (now - at) as f32 / dur as f32;
    let decay = 1.0 - t;
    let phase = (t * 6.0 * std::f32::consts::PI).sin();
    (phase * decay * 1.5).round() as i8
}

/// How strongly each glyph in a comet tail leans toward the caret color.
/// The caret end gets the full `gain`, fading back through the tail.
pub fn comet_weights(cols: usize, gain: f32) -> Vec<f32> {
    (0..cols)
        .map(|i| {
            let d = (cols - 1 - i) as f32 / cols.max(1) as f32;
            (1.0 - d).powf(2.2) * gain
        })
        .collect()
}

/// Alpha multiplier for a row under a fade mask at the top of a region.
/// Row 0 is the topmost and therefore the faintest.
pub fn top_mask(row: usize, rows: usize, strength: f32) -> f32 {
    if rows == 0 {
        return 1.0;
    }
    let d = 1.0 - (row as f32 / rows as f32);
    (1.0 - d * strength).clamp(0.0, 1.0)
}

/// A skeleton placeholder row: faint cells with a bright band travelling
/// through them, so the wait before the first token has something alive in it.
#[allow(dead_code)]
pub fn skeleton_colors(cols: usize, phase: f32, width: f32, base: Rgb, hot: Rgb) -> Vec<Rgb> {
    let center = phase * cols as f32;
    (0..cols)
        .map(|i| {
            let w = bell(i as f32, center, width);
            lerp(base, hot, 0.16 + w * 0.5)
        })
        .collect()
}

/// A text color where a bright band travels across `cols` characters.
/// `phase` is 0..1 across the whole span; cells near it get brighter.
#[allow(dead_code)]
pub fn shimmer_colors(
    base: Rgb,
    hot: Rgb,
    cols: usize,
    phase: f32,
    width: f32,
) -> Vec<Rgb> {
    let center = phase * cols as f32;
    (0..cols)
        .map(|i| {
            let w = bell(i as f32, center, width);
            lerp(base, hot, w * 0.9)
        })
        .collect()
}

/// Sweep a gradient across a run of cells, optionally travelling with time.
pub fn sweep_colors(stops: &[Rgb], cols: usize, offset: f32) -> Vec<Rgb> {
    (0..cols)
        .map(|i| {
            let t = if cols <= 1 {
                0.0
            } else {
                (i as f32 / (cols - 1) as f32 + offset).fract()
            };
            crate::theme::ramp(stops, t)
        })
        .collect()
}

/// Number of lines to show for a height reveal (tool cards opening up).
pub fn reveal_lines(total: usize, t: f32) -> usize {
    ((total as f32 * t).ceil() as usize).clamp(0, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tween_respects_delay_and_duration() {
        let tw = Tween::delayed(100, 200, Ease::Linear);
        assert_eq!(tw.t(50, 0), 0.0);
        assert!((tw.t(200, 0) - 0.5).abs() < 1e-6);
        assert_eq!(tw.t(400, 0), 1.0);
        assert!(tw.done(300, 0));
        assert!(!tw.done(299, 0));
    }

    #[test]
    fn eases_hit_endpoints() {
        for e in [Ease::Linear, Ease::OutCubic, Ease::InOutSine, Ease::OutBack] {
            assert!((ease(e, 0.0) - 0.0).abs() < 1e-6, "{e:?} start");
            assert!((ease(e, 1.0) - 1.0).abs() < 1e-6, "{e:?} end");
        }
    }

    #[test]
    fn reveal_is_monotonic() {
        assert_eq!(reveal_lines(10, 0.0), 0);
        assert_eq!(reveal_lines(10, 1.0), 10);
        let mut last = 0;
        for i in 0..=10 {
            let n = reveal_lines(10, i as f32 / 10.0);
            assert!(n >= last);
            last = n;
        }
    }

    #[test]
    fn bell_peaks_at_center() {
        assert!(bell(5.0, 5.0, 1.0) > bell(7.0, 5.0, 1.0));
        assert!((bell(5.0, 5.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn anim_walks_to_target() {
        let mut a = Anim::at(0.0);
        assert_eq!(a.get(0), 0.0);
        a.set(100.0, 1_000, 400);
        assert_eq!(a.get(1_000), 0.0);
        let mid = a.get(1_200);
        assert!(mid > 0.0 && mid < 100.0, "mid={mid}");
        assert_eq!(a.get(5_000), 100.0);
        assert!(a.done(5_000));
        assert!(!a.done(1_100));
    }

    #[test]
    fn anim_without_movement_is_stable() {
        let a = Anim::at(42.0);
        assert_eq!(a.get(0), 42.0);
        assert_eq!(a.get(9_999), 42.0);
        assert!(a.done(9_999));
    }

    #[test]
    fn shake_only_inside_its_window() {
        assert_eq!(shake(100, 200, 300), 0);
        assert_eq!(shake(600, 200, 300), 0);
        assert!(shake(210, 200, 300).abs() <= 2);
        assert!(shake(350, 200, 300).abs() <= 2);
    }

    #[test]
    fn comet_is_brightest_at_the_caret() {
        let weights = comet_weights(8, 1.0);
        assert_eq!(weights.len(), 8);
        assert!(weights[7] > weights[3]);
        assert!(weights[3] > weights[0]);
        assert!((weights[7] - 1.0).abs() < 1e-6, "caret end takes the full gain");
        for w in comet_weights(6, 0.5) {
            assert!(w <= 0.5, "gain caps the tail");
        }
    }

    #[test]
    fn top_mask_fades_upward() {
        assert!(top_mask(0, 3, 0.8) < top_mask(1, 3, 0.8));
        assert!(top_mask(1, 3, 0.8) < top_mask(2, 3, 0.8));
        assert_eq!(top_mask(2, 3, 0.0), 1.0);
    }

    #[test]
    fn skeleton_stays_within_palette() {
        for c in skeleton_colors(20, 0.5, 3.0, (40, 40, 40), (200, 200, 200)) {
            assert!(c.0 >= 40 && c.0 <= 200, "{c:?}");
        }
    }
}
