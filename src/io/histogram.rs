//! Histogram computation for the preview (PRD 7.5).
//!
//! Histograms are computed from the cached preview image (the long-edge-1920
//! JPEG) so they are ready once the preview is generated. To keep single-photo
//! cost well under the <15ms budget, the image is downsampled before binning so
//! we only bin a few tens of thousands of samples.

/// A 256-bin per-channel histogram (R/G/B/L). `total` is the number of sampled
/// pixels (used for overflow percentage).
#[derive(Debug, Clone)]
pub struct Histogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    pub l: [u32; 256],
    pub total: u32,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::zero()
    }
}

impl Histogram {
    pub fn zero() -> Self {
        Self {
            r: [0u32; 256],
            g: [0u32; 256],
            b: [0u32; 256],
            l: [0u32; 256],
            total: 0,
        }
    }

    /// Compute a histogram from a decoded image, downsampling so the sample
    /// count stays small and the computation is fast.
    pub fn from_image(img: &image::DynamicImage) -> Self {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        if w == 0 || h == 0 {
            return Self::zero();
        }
        // Cap the sampled long edge so binning stays cheap (<15ms).
        const MAX_SAMPLE_EDGE: u32 = 320;
        let long = w.max(h);
        let small = if long > MAX_SAMPLE_EDGE {
            let scale = MAX_SAMPLE_EDGE as f64 / long as f64;
            let nw = ((w as f64) * scale).round().max(1.0) as u32;
            let nh = ((h as f64) * scale).round().max(1.0) as u32;
            image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Triangle)
        } else {
            rgb
        };

        let mut hist = Self::zero();
        for p in small.pixels() {
            let [r, g, b] = p.0;
            let ri = r as usize;
            let gi = g as usize;
            let bi = b as usize;
            hist.r[ri] += 1;
            hist.g[gi] += 1;
            hist.b[bi] += 1;
            // Rec.709 luminance, clamped to 0..255.
            let l = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32).round();
            let li = (l as i32).clamp(0, 255) as usize;
            hist.l[li] += 1;
            hist.total += 1;
        }
        hist
    }

    /// Load the cached preview image for a hash and compute its histogram.
    /// Returns None when the preview cache does not exist yet.
    pub fn from_preview_cache(hash: &str) -> Option<Self> {
        let path = crate::io::thumbnails::preview_path(hash);
        if !path.exists() {
            return None;
        }
        let img = image::open(&path).ok()?;
        Some(Self::from_image(&img))
    }

    /// Fraction (0..1) of samples at or near the lowest channel value (black).
    pub fn black_ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let n = self.l[0] + self.l[1] + self.l[2];
        n as f32 / self.total as f32
    }

    /// Fraction (0..1) of samples at or near the highest channel value (white).
    pub fn white_ratio(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let n = self.l[253] + self.l[254] + self.l[255];
        n as f32 / self.total as f32
    }
}
