use eframe::egui;

/// Parsed, single-envelope waveform data returned by blaze-engine.
pub struct Waveform {
    min: Vec<i16>,
    max: Vec<i16>,
}

impl Waveform {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 16 {
            return Err("Waveform data is shorter than its header.".to_owned());
        }

        let read_u32 = |offset: usize| {
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("validated header"))
        };

        if read_u32(0) != 0x4257_5631 {
            return Err("Waveform data has an unknown format.".to_owned());
        }
        if read_u32(4) != 1 {
            return Err("Waveform data must contain exactly one merged lane.".to_owned());
        }

        let blocks = read_u32(12) as usize;
        let expected_len = 16_usize
            .checked_add(blocks.checked_mul(6).ok_or("Waveform size overflow.")?)
            .ok_or("Waveform size overflow.")?;
        if bytes.len() != expected_len {
            return Err("Waveform data has an invalid length.".to_owned());
        }

        let min_start = 16;
        let max_start = min_start + blocks * 2;
        let min = (0..blocks)
            .map(|index| {
                i16::from_le_bytes([
                    bytes[min_start + index * 2],
                    bytes[min_start + index * 2 + 1],
                ])
            })
            .collect();
        let max = (0..blocks)
            .map(|index| {
                i16::from_le_bytes([
                    bytes[max_start + index * 2],
                    bytes[max_start + index * 2 + 1],
                ])
            })
            .collect();

        if blocks == 0 {
            return Err("The audio stream contains no waveform samples.".to_owned());
        }

        Ok(Self { min, max })
    }

    /// Paint one continuous, mirrored amplitude silhouette. Samples are
    /// reduced to the strongest peak within each horizontal screen pixel.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        if self.min.is_empty() || rect.width() < 1.0 || rect.height() < 2.0 {
            return;
        }

        let painter = painter.with_clip_rect(rect);
        let baseline = rect.center().y;
        let half_height = rect.height() * 0.47;
        let width = rect.width().ceil() as usize;
        let count = self.min.len();

        painter.line_segment(
            [
                egui::pos2(rect.left(), baseline),
                egui::pos2(rect.right(), baseline),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 59, 66)),
        );

        let waveform_color = egui::Color32::from_rgb(174, 139, 119);
        let stroke = egui::Stroke::new(1.0, waveform_color);

        for column in 0..width {
            let start = column * count / width;
            let end = (((column + 1) * count + width - 1) / width).max(start + 1).min(count);
            if start >= count {
                break;
            }

            let mut peak = 0_i32;
            for index in start..end {
                peak = peak
                    .max((self.min[index] as i32).abs())
                    .max((self.max[index] as i32).abs());
            }

            if peak == 0 {
                continue;
            }

            let amplitude = (peak as f32 / 32768.0).min(1.0) * half_height;
            let x = (rect.left() + column as f32 + 0.5).min(rect.right());
            painter.line_segment(
                [
                    egui::pos2(x, baseline - amplitude),
                    egui::pos2(x, baseline + amplitude),
                ],
                stroke,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_lane_peak_data() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x4257_5631_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&200_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&(-100_i16).to_le_bytes());
        bytes.extend_from_slice(&(-200_i16).to_le_bytes());
        bytes.extend_from_slice(&100_i16.to_le_bytes());
        bytes.extend_from_slice(&200_i16.to_le_bytes());
        bytes.extend_from_slice(&50_u16.to_le_bytes());
        bytes.extend_from_slice(&100_u16.to_le_bytes());

        let waveform = Waveform::from_bytes(&bytes).unwrap();
        assert_eq!(waveform.min, vec![-100, -200]);
        assert_eq!(waveform.max, vec![100, 200]);
    }

    #[test]
    fn rejects_invalid_header_and_truncated_data() {
        assert!(Waveform::from_bytes(&[]).is_err());
        let mut bytes = vec![0; 16];
        assert!(Waveform::from_bytes(&bytes).is_err());
        bytes[..4].copy_from_slice(&0x4257_5631_u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&1_u32.to_le_bytes());
        assert!(Waveform::from_bytes(&bytes).is_err());
    }
}
