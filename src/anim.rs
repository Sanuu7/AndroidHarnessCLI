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

/// A text color where a bright band travels across `cols` characters.
/// `phase` is 0..1 across the whole span; cells near it get brighter.
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
}
