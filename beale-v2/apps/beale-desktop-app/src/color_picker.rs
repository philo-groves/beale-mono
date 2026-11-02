#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hsv {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

impl Hsv {
    pub(crate) fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        let channel = |start| {
            u8::from_str_radix(hex.get(start..start + 2)?, 16)
                .ok()
                .map(|n| f32::from(n) / 255.0)
        };
        let r = channel(0)?;
        let g = channel(2)?;
        let b = channel(4)?;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let hue = if delta == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        Some(Self {
            hue,
            saturation: if max == 0.0 { 0.0 } else { delta / max },
            value: max,
        })
    }

    pub(crate) fn hex(self) -> String {
        let h = self.hue.rem_euclid(1.0) * 6.0;
        let c = self.value.clamp(0.0, 1.0) * self.saturation.clamp(0.0, 1.0);
        let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
        let (r, g, b) = match h as u8 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = self.value.clamp(0.0, 1.0) - c;
        let byte = |value: f32| ((value + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
    }
}

#[cfg(test)]
mod tests {
    use super::Hsv;

    #[test]
    fn hex_and_hsv_round_trip() {
        for hex in [
            "#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff", "#123456", "#e6ce7f",
        ] {
            assert_eq!(Hsv::from_hex(hex).unwrap().hex(), hex);
        }
        assert_eq!(Hsv::from_hex("#abc"), None);
        assert_eq!(Hsv::from_hex("#gggggg"), None);
        assert_eq!(Hsv::from_hex("#aéabc"), None);
        assert_eq!(
            Hsv {
                hue: 1.0,
                saturation: 1.0,
                value: 1.0
            }
            .hex(),
            "#ff0000"
        );
    }
}
