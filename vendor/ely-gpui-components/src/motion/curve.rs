use std::time::Duration;

use gpui::{App, Pixels, px};

use crate::theme::ActiveTheme;

pub const FAST: Duration = Duration::from_millis(120);
pub const BASE: Duration = Duration::from_millis(200);
pub const SLOW: Duration = Duration::from_millis(320);
pub const THEME: Duration = Duration::from_millis(280);

/// Travel of small entrances.
pub const NUDGE: Pixels = px(4.0);

const DAMPING: f32 = 0.75;
const OMEGA: f32 = 6.91 / DAMPING;

/// `base`, or one millisecond under reduced motion.
pub fn duration(base: Duration, cx: &App) -> Duration {
    if cx.theme().reduced_motion {
        Duration::from_millis(1)
    } else {
        base
    }
}

pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` at `t`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let at = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let error = at(x1, x2, s) - t;
        if error.abs() < 1e-5 {
            break;
        }
        let dx = slope(x1, x2, s);
        if dx.abs() < 1e-6 {
            break;
        }
        s = (s - error / dx).clamp(0.0, 1.0);
    }
    at(y1, y2, s)
}

/// CSS `ease-out`, the web app's soft fade.
pub fn ease_out_css(t: f32) -> f32 {
    cubic_bezier(0.0, 0.0, 0.58, 1.0, t)
}

/// CSS `cubic-bezier(0.22, 1, 0.36, 1)`, the web app's entrance and exit curve.
pub fn ease_out_emphasis(t: f32) -> f32 {
    cubic_bezier(0.22, 1.0, 0.36, 1.0, t)
}

/// Damped spring. Overshoots; not a gpui easing.
pub fn spring(t: f32) -> f32 {
    if t >= 1.0 {
        return 1.0;
    }
    let damped = OMEGA * (1.0 - DAMPING * DAMPING).sqrt();
    let decay = (-DAMPING * OMEGA * t).exp();
    1.0 - decay * ((damped * t).cos() + DAMPING * OMEGA / damped * (damped * t).sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_pin_their_ends() {
        for ease in [ease_out_cubic, ease_in_out_cubic, spring] {
            assert!(ease(0.0).abs() < 1e-6);
            assert_eq!(ease(1.0), 1.0);
        }
    }

    #[test]
    fn spring_overshoots_a_little_then_settles() {
        let peak = (0..=100)
            .map(|i| spring(i as f32 / 100.0))
            .fold(0.0, f32::max);
        assert!(peak > 1.01 && peak < 1.05, "peak {peak}");
        assert!((spring(0.95) - 1.0).abs() < 0.01);
    }

    #[test]
    fn ease_in_out_is_symmetric() {
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!((ease_in_out_cubic(t) + ease_in_out_cubic(1.0 - t) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn css_easings_pin_their_ends_and_rise() {
        let easings: [fn(f32) -> f32; 2] = [ease_out_css, ease_out_emphasis];
        for ease in easings {
            assert_eq!(ease(0.0), 0.0);
            assert_eq!(ease(1.0), 1.0);
            let values: Vec<f32> = (0..=20).map(|i| ease(i as f32 / 20.0)).collect();
            assert!(values.windows(2).all(|pair| pair[1] >= pair[0]));
        }
    }
}
