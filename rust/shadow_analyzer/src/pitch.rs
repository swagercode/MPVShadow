// Minimal MPM (NSDF-based) pitch estimation for short offline clips.
// Pure Rust, no external deps. Optimized for clarity and acceptable speed
// on small frames (30–40 ms) and modest tau ranges (80–350 Hz).

#[derive(Clone, Copy, Debug)]
pub struct F0Config {
    pub sample_rate_hz: f32,
    pub frame_size: usize,
    pub hop_size: usize,
    pub fmin_hz: f32,
    pub fmax_hz: f32,
    pub nsdf_threshold: f32,
}

impl Default for F0Config {
    fn default() -> Self {
        // Defaults: 24 kHz, 40 ms frame, 10 ms hop, range 80–350 Hz
        let sr = 24000.0f32;
        let frame = (0.040 * sr) as usize; // 40 ms
        let hop = (0.010 * sr) as usize;   // 10 ms
        Self {
            sample_rate_hz: sr,
            frame_size: frame.max(1),
            hop_size: hop.max(1),
            fmin_hz: 70.0,
            fmax_hz: 350.0,
            nsdf_threshold: 0.40,
        }
    }
}

#[derive(Debug, Default)]
pub struct F0Result {
    pub f0_hz: Vec<f32>,      // 0.0 for unvoiced
    pub voiced_flags: Vec<bool>,
    pub median_hz: Option<f32>,
    pub voiced_ratio: f32,
}

pub fn estimate_f0_mpm(samples: &[f32], cfg: &F0Config) -> F0Result {
    if samples.is_empty() || cfg.frame_size < 3 {
        return F0Result::default();
    }
    let sr = cfg.sample_rate_hz.max(1.0);
    let tau_min = ((sr / cfg.fmax_hz.max(1.0)).floor() as usize).max(2);
    let tau_max = ((sr / cfg.fmin_hz.max(1.0)).ceil() as usize).max(tau_min + 1);
    let nsdf_thresh = cfg.nsdf_threshold.clamp(0.0, 1.0);

    let frame_size = cfg.frame_size;
    let hop = cfg.hop_size.max(1);
    let mut nsdf: Vec<f32> = vec![0.0; tau_max + 1];

    let mut f0_series: Vec<f32> = Vec::new();
    let mut voiced_flags: Vec<bool> = Vec::new();

    let mut start = 0usize;
    while start + frame_size <= samples.len() {
        let frame = &samples[start..start + frame_size];

        // Compute NSDF(tau) for tau in [tau_min, tau_max]
        // NSDF(tau) = 2 * sum_j x_j x_{j+tau} / (sum_j x_j^2 + x_{j+tau}^2)
        // where j runs so that indices are valid within frame.
        for tau in tau_min..=tau_max {
            let limit = frame_size - tau;
            if limit < 2 {
                nsdf[tau] = 0.0;
                continue;
            }
            let mut num: f64 = 0.0;
            let mut den: f64 = 0.0;
            // Naive loops; frame sizes are small (<= ~1024).
            for j in 0..limit {
                let a = frame[j] as f64;
                let b = frame[j + tau] as f64;
                num += a * b;
                den += a * a + b * b;
            }
            let v = if den > 0.0 { (2.0 * num / den) as f32 } else { 0.0 };
            nsdf[tau] = v;
        }

        // Peak picking: choose highest local max between tau_min..tau_max.
        // Optional refinement: parabolic interpolation around the best peak.
        let mut best_tau = 0usize;
        let mut best_val = -1.0f32;
        for tau in (tau_min + 1)..tau_max {
            let prev = nsdf[tau - 1];
            let cur = nsdf[tau];
            let next = nsdf[tau + 1];
            if cur > prev && cur >= next && cur > best_val {
                best_val = cur;
                best_tau = tau;
            }
        }

        let mut f0_hz = 0.0f32;
        let mut voiced = false;
        if best_tau >= tau_min && best_tau <= tau_max && best_val >= nsdf_thresh {
            // Parabolic interpolation for sub-sample tau (optional)
            let l = nsdf[best_tau - 1];
            let c = nsdf[best_tau];
            let r = nsdf[best_tau + 1];
            let denom = (l - 2.0 * c + r);
            let delta = if denom.abs() > 1e-12 { 0.5 * (l - r) / denom } else { 0.0 };
            let tau_refined = (best_tau as f32 + delta).max(tau_min as f32).min(tau_max as f32);
            let freq = sr / tau_refined.max(1.0);
            if freq.is_finite() && freq > 0.0 {
                f0_hz = freq;
                voiced = true;
            }
        }

        f0_series.push(if voiced { f0_hz } else { 0.0 });
        voiced_flags.push(voiced);

        start += hop;
    }

    // Compute median over voiced frames
    let mut voiced_vals: Vec<f32> = f0_series.iter().copied().filter(|&x| x > 0.0).collect();
    voiced_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if voiced_vals.is_empty() {
        None
    } else {
        let mid = voiced_vals.len() / 2;
        if voiced_vals.len() % 2 == 1 {
            Some(voiced_vals[mid])
        } else {
            Some(0.5 * (voiced_vals[mid - 1] + voiced_vals[mid]))
        }
    };

    let voiced_ratio = if !voiced_flags.is_empty() {
        let v = voiced_flags.iter().filter(|&&b| b).count() as f32;
        v / (voiced_flags.len() as f32)
    } else { 0.0 };

    F0Result { f0_hz: f0_series, voiced_flags, median_hz: median, voiced_ratio }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_sine(sr: f32, freq: f32, secs: f32) -> Vec<f32> {
        let n = (sr * secs) as usize;
        let mut out = Vec::with_capacity(n);
        let dt = 1.0 / sr;
        let mut t = 0.0f32;
        for _ in 0..n {
            out.push((2.0 * std::f32::consts::PI * freq * t).sin() * 0.5);
            t += dt;
        }
        out
    }

    #[test]
    fn test_sine_200hz_ok() {
        let sr = 24000.0;
        let sig = gen_sine(sr, 200.0, 0.5);
        let mut cfg = F0Config::default();
        cfg.sample_rate_hz = sr;
        let res = estimate_f0_mpm(&sig, &cfg);
        assert!(res.voiced_ratio > 0.7, "voiced_ratio={}", res.voiced_ratio);
        let med = res.median_hz.expect("median");
        assert!((med - 200.0).abs() < 3.0, "median={}", med);
    }

    #[test]
    fn test_silence_unvoiced() {
        let sr = 24000.0;
        let sig = vec![0.0f32; (sr as usize) / 2];
        let cfg = F0Config::default();
        let res = estimate_f0_mpm(&sig, &cfg);
        assert!(res.median_hz.is_none());
        assert!(res.voiced_ratio < 0.05);
    }
}


