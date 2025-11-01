use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use anyhow::{Result, Context};

#[derive(Debug, Clone, Copy)]
pub struct WavInfo {
	pub sample_rate: u32,
	pub channels: u16,
	pub bits_per_sample: u16,
}

// Read minimal PCM 16-bit WAV and return mono f32 samples in [-1, 1] and the (possibly new) sample rate.
// If target_sample_rate is Some(24000), performs simple 2x decimation when input is 48000 Hz.
pub fn read_wav_mono_16bit(path: &Path, target_sample_rate: Option<u32>) -> Result<(Vec<f32>, u32)> {
	let mut f = File::open(path).with_context(|| format!("open wav: {}", path.display()))?;
	let mut buf = Vec::new();
	f.read_to_end(&mut buf).with_context(|| "read wav bytes")?;

	let (info, data_off, data_len) = parse_header_minimal(&buf)?;
	if info.bits_per_sample != 16 {
		anyhow::bail!("unsupported bits_per_sample: {}", info.bits_per_sample);
	}
	if data_off + data_len > buf.len() {
		anyhow::bail!("wav data chunk out of bounds");
	}
	let bytes = &buf[data_off..data_off + data_len];
	let total_samples = (bytes.len() / 2) as usize; // i16 samples interleaved
	if total_samples == 0 { return Ok((Vec::new(), target_sample_rate.unwrap_or(info.sample_rate))); }

	let ch = info.channels.max(1);
	let frames = total_samples / ch as usize;
	let mut mono: Vec<f32> = Vec::with_capacity(frames);
	let mut i = 0usize;
	for _ in 0..frames {
		let mut acc: f32 = 0.0;
		for _c in 0..ch {
			let lo = bytes[i] as u16 as u32;
			let hi = bytes[i + 1] as i8 as i32 as i64; // sign-extend via i8 -> i32 -> i64
			let sample_i16 = ((hi as i32) << 8) | (lo as i32);
			let sample = (sample_i16 as f32) / 32768.0;
			acc += sample;
			i += 2;
		}
		mono.push(acc / (ch as f32));
	}

	let out_sr = if let Some(tgt) = target_sample_rate { tgt } else { info.sample_rate };
	if info.sample_rate == 48000 && out_sr == 24000 {
		// 2x decimation with simple 2-tap averaging to reduce aliasing
		let mut dec: Vec<f32> = Vec::with_capacity(mono.len() / 2 + 1);
		let mut j = 0usize;
		while j + 1 < mono.len() {
			let v = 0.5 * (mono[j] + mono[j + 1]);
			dec.push(v);
			j += 2;
		}
		Ok((dec, out_sr))
	} else {
		Ok((mono, info.sample_rate))
	}
}

fn parse_header_minimal(buf: &[u8]) -> Result<(WavInfo, usize, usize)> {
	if buf.len() < 44 { anyhow::bail!("wav too small"); }
	if &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" { anyhow::bail!("not RIFF/WAVE"); }
	let mut p = 12usize;
	let mut info: Option<WavInfo> = None;
	let mut data_off: Option<usize> = None;
	let mut data_len: Option<usize> = None;
	while p + 8 <= buf.len() {
		let chunk_id = &buf[p..p + 4];
		let chunk_size = u32::from_le_bytes([buf[p + 4], buf[p + 5], buf[p + 6], buf[p + 7]]) as usize;
		let payload_off = p + 8;
		if payload_off + chunk_size > buf.len() { anyhow::bail!("chunk OOB"); }
		if chunk_id == b"fmt " {
			if chunk_size < 16 { anyhow::bail!("fmt too small"); }
			let audio_format = u16::from_le_bytes([buf[payload_off], buf[payload_off + 1]]);
			let channels = u16::from_le_bytes([buf[payload_off + 2], buf[payload_off + 3]]);
			let sample_rate = u32::from_le_bytes([
				buf[payload_off + 4], buf[payload_off + 5], buf[payload_off + 6], buf[payload_off + 7]
			]);
			let bits_per_sample = u16::from_le_bytes([
				buf[payload_off + 14], buf[payload_off + 15]
			]);
			if audio_format != 1 { anyhow::bail!("unsupported format {} (PCM only)", audio_format); }
			info = Some(WavInfo { sample_rate, channels, bits_per_sample });
		} else if chunk_id == b"data" {
			data_off = Some(payload_off);
			data_len = Some(chunk_size);
		}
		p = payload_off + chunk_size;
	}
	let info = info.context("missing fmt chunk")?;
	let data_off = data_off.context("missing data chunk")?;
	let data_len = data_len.context("missing data length")?;
	Ok((info, data_off, data_len))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs; 
	use std::path::PathBuf;

	fn write_test_wav_i16(path: &Path, sr: u32, channels: u16, pcm: &[i16]) -> Result<()> {
		let mut f = File::create(path).context("create wav")?;
		let byte_len = (pcm.len() * 2) as u32;
		let block_align = channels * 2;
		let byte_rate = sr * block_align as u32;
		let riff_size = 36 + byte_len;
		// RIFF header
		f.write_all(b"RIFF")?;
		f.write_all(&riff_size.to_le_bytes())?;
		f.write_all(b"WAVE")?;
		// fmt chunk
		f.write_all(b"fmt ")?;
		f.write_all(&16u32.to_le_bytes())?; // PCM fmt chunk size
		f.write_all(&1u16.to_le_bytes())?; // PCM
		f.write_all(&channels.to_le_bytes())?;
		f.write_all(&sr.to_le_bytes())?;
		f.write_all(&byte_rate.to_le_bytes())?;
		f.write_all(&block_align.to_le_bytes())?;
		f.write_all(&16u16.to_le_bytes())?; // bits per sample
		// data chunk
		f.write_all(b"data")?;
		f.write_all(&byte_len.to_le_bytes())?;
		// interleaved samples already in pcm
		let mut bytes = vec![0u8; pcm.len() * 2];
		for (i, s) in pcm.iter().enumerate() { let b = s.to_le_bytes(); bytes[2*i] = b[0]; bytes[2*i+1] = b[1]; }
		f.write_all(&bytes)?;
		Ok(())
	}

	#[test]
	fn test_read_wav_stereo_48k_to_mono_24k() {
		let sr = 48000u32;
		let n = 4800usize; // 0.1 s frames
		let mut interleaved: Vec<i16> = Vec::with_capacity(n * 2);
		for i in 0..n {
			let s = (((i as f32 / n as f32) * 2.0 - 1.0) * 0.5 * 32767.0) as i16;
			// stereo identical
			interleaved.push(s);
			interleaved.push(s);
		}
		let mut tmp = std::env::temp_dir();
		tmp.push("test_stereo.wav");
		let _ = fs::remove_file(&tmp);
		write_test_wav_i16(&tmp, sr, 2, &interleaved).unwrap();
		let (mono, out_sr) = read_wav_mono_16bit(&tmp, Some(24000)).unwrap();
		assert_eq!(out_sr, 24000);
		assert_eq!(mono.len(), n / 2);
		let _ = fs::remove_file(&tmp);
	}

	#[test]
	fn test_read_wav_mono_passthrough() {
		let sr = 48000u32;
		let n = 3200usize;
		let mut mono_i16: Vec<i16> = Vec::with_capacity(n);
		for i in 0..n { mono_i16.push(((i as f32 / n as f32) * 2.0 - 1.0) as f32 as i16); }
		let mut p = PathBuf::from(std::env::temp_dir());
		p.push("test_mono.wav");
		let _ = fs::remove_file(&p);
		write_test_wav_i16(&p, sr, 1, &mono_i16).unwrap();
		let (mono, out_sr) = read_wav_mono_16bit(&p, None).unwrap();
		assert_eq!(out_sr, sr);
		assert_eq!(mono.len(), n);
		let _ = fs::remove_file(&p);
	}
}


