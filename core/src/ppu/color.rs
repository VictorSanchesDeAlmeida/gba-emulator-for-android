/// Converts a GBA 15-bit color (`0BBBBBGGGGGRRRRR`) to 8-bit-per-channel
/// RGBA, for output to a native renderer / test assertions. Each 5-bit
/// channel is expanded to 8 bits by replicating its top 3 bits into the
/// low bits (`(c << 3) | (c >> 2)`), the standard bit-replication scale
/// that maps 0x1F -> 0xFF exactly instead of leaving the top end short.
pub fn bgr555_to_rgba8888(color: u16) -> [u8; 4] {
    let r5 = (color & 0x1F) as u8;
    let g5 = ((color >> 5) & 0x1F) as u8;
    let b5 = ((color >> 10) & 0x1F) as u8;
    let scale = |c: u8| (c << 3) | (c >> 2);
    [scale(r5), scale(g5), scale(b5), 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white_round_trip_exactly() {
        assert_eq!(bgr555_to_rgba8888(0x0000), [0, 0, 0, 255]);
        assert_eq!(bgr555_to_rgba8888(0x7FFF), [255, 255, 255, 255]);
    }

    #[test]
    fn pure_red_channel() {
        assert_eq!(bgr555_to_rgba8888(0x001F), [255, 0, 0, 255]);
    }

    #[test]
    fn pure_blue_channel_is_the_high_bits() {
        assert_eq!(bgr555_to_rgba8888(0x7C00), [0, 0, 255, 255]);
    }
}
