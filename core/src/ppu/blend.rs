//! Color special effects math: alpha blending between two BGR555 colors,
//! and brightness increase/decrease of one. Coefficients (EVA/EVB/EVY) are
//! 0-16 in 1/16ths, matching real hardware's fixed-point blend math.

fn split(c: u16) -> (u16, u16, u16) {
    (c & 0x1F, (c >> 5) & 0x1F, (c >> 10) & 0x1F)
}

fn combine(r: u16, g: u16, b: u16) -> u16 {
    r | (g << 5) | (b << 10)
}

pub(super) fn alpha_blend(c1: u16, c2: u16, eva: u8, evb: u8) -> u16 {
    let mix = |a: u16, b: u16| -> u16 { ((a * eva as u16 + b * evb as u16) / 16).min(31) };
    let (r1, g1, b1) = split(c1);
    let (r2, g2, b2) = split(c2);
    combine(mix(r1, r2), mix(g1, g2), mix(b1, b2))
}

pub(super) fn brightness_increase(c: u16, evy: u8) -> u16 {
    let ch = |v: u16| -> u16 { (v + (31 - v) * evy as u16 / 16).min(31) };
    let (r, g, b) = split(c);
    combine(ch(r), ch(g), ch(b))
}

pub(super) fn brightness_decrease(c: u16, evy: u8) -> u16 {
    let ch = |v: u16| -> u16 { v.saturating_sub(v * evy as u16 / 16) };
    let (r, g, b) = split(c);
    combine(ch(r), ch(g), ch(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_blend_full_weight_on_first_color_is_a_no_op() {
        let red = 0x001F;
        let blue = 0x7C00;
        assert_eq!(alpha_blend(red, blue, 16, 0), red);
    }

    #[test]
    fn alpha_blend_averages_at_half_weight_each() {
        // Red (31,0,0) blended 50/50 with white (31,31,31) -> (31,15,15).
        let red = 0x001F;
        let white = 0x7FFF;
        assert_eq!(alpha_blend(red, white, 8, 8), combine(31, 15, 15));
    }

    #[test]
    fn alpha_blend_clamps_past_full_intensity() {
        // eva+evb summing above 16 can overshoot 31 per channel; must clamp.
        let white = 0x7FFF;
        assert_eq!(alpha_blend(white, white, 16, 16), white);
    }

    #[test]
    fn brightness_increase_at_max_coefficient_reaches_white() {
        let red = 0x001F;
        assert_eq!(brightness_increase(red, 16), 0x7FFF);
    }

    #[test]
    fn brightness_increase_at_zero_coefficient_is_a_no_op() {
        let red = 0x001F;
        assert_eq!(brightness_increase(red, 0), red);
    }

    #[test]
    fn brightness_decrease_at_max_coefficient_reaches_black() {
        let white = 0x7FFF;
        assert_eq!(brightness_decrease(white, 16), 0x0000);
    }

    #[test]
    fn brightness_decrease_at_zero_coefficient_is_a_no_op() {
        let white = 0x7FFF;
        assert_eq!(brightness_decrease(white, 0), white);
    }
}
