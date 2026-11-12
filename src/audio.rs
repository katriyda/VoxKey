//! Microphone capture and linear resampling used by the ASR worker.

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::i18n::{trf, ts};

pub struct Recorder {
    stream: Option<cpal::Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
    error: Arc<Mutex<Option<String>>>,
    sample_rate: u32,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| {
            ts!(
                "没有可用的麦克风输入设备",
                "No microphone input device is available"
            )
            .to_string()
        })?;
        let device_name = {
            let name = device.to_string();
            if name.trim().is_empty() {
                ts!("默认麦克风", "Default microphone").to_string()
            } else {
                name
            }
        };
        let supported = device.default_input_config().map_err(|e| {
            trf!(
                "无法读取麦克风配置: {e}",
                "Cannot read the microphone configuration: {e}"
            )
        })?;
        let sample_format = supported.sample_format();
        let config = supported.config();
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;
        if channels == 0 || sample_rate == 0 {
            return Err(ts!(
                "麦克风返回了无效的音频参数",
                "The microphone reported invalid audio parameters"
            )
            .to_string());
        }

        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let level = Arc::new(AtomicU32::new(0));
        let error = Arc::new(Mutex::new(None::<String>));

        let stream = match sample_format {
            SampleFormat::F32 => {
                let buffer = Arc::clone(&buffer);
                let level = Arc::clone(&level);
                let err_fn = make_error_callback(Arc::clone(&error), device_name.clone());
                device.build_input_stream(
                    config,
                    move |data: &[f32], _| push_mono(data, channels, &buffer, &level, |s| s),
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let buffer = Arc::clone(&buffer);
                let level = Arc::clone(&level);
                let err_fn = make_error_callback(Arc::clone(&error), device_name.clone());
                device.build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        push_mono(data, channels, &buffer, &level, |s| s as f32 / 32768.0)
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let buffer = Arc::clone(&buffer);
                let level = Arc::clone(&level);
                let err_fn = make_error_callback(Arc::clone(&error), device_name.clone());
                device.build_input_stream(
                    config,
                    move |data: &[u16], _| {
                        push_mono(data, channels, &buffer, &level, |s| {
                            (s as f32 - 32768.0) / 32768.0
                        })
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::I32 => {
                let buffer = Arc::clone(&buffer);
                let level = Arc::clone(&level);
                let err_fn = make_error_callback(Arc::clone(&error), device_name.clone());
                device.build_input_stream(
                    config,
                    move |data: &[i32], _| {
                        push_mono(data, channels, &buffer, &level, |s| {
                            s as f32 / 2_147_483_648.0
                        })
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U8 => {
                let buffer = Arc::clone(&buffer);
                let level = Arc::clone(&level);
                let err_fn = make_error_callback(Arc::clone(&error), device_name.clone());
                device.build_input_stream(
                    config,
                    move |data: &[u8], _| {
                        push_mono(data, channels, &buffer, &level, |s| {
                            (s as f32 - 128.0) / 128.0
                        })
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                return Err(trf!(
                    "当前麦克风的采样格式不受支持: {other:?}",
                    "The current microphone's sample format is not supported: {other:?}"
                ));
            }
        }
        .map_err(|e| trf!("无法打开麦克风: {e}", "Cannot open the microphone: {e}"))?;

        stream
            .play()
            .map_err(|e| trf!("无法开始录音: {e}", "Cannot start recording: {e}"))?;

        Ok(Self {
            stream: Some(stream),
            buffer,
            level,
            error,
            sample_rate,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn snapshot(&self) -> Vec<f32> {
        self.buffer
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Copies `from..to` (raw device-rate samples), clamped to what has been
    /// captured. Live preview only needs the tail of the recording, so this
    /// avoids copying the whole buffer every few hundred milliseconds.
    pub fn snapshot_range(&self, from: usize, to: usize) -> Vec<f32> {
        self.buffer
            .lock()
            .map(|guard| {
                let end = to.min(guard.len());
                let start = from.min(end);
                guard[start..end].to_vec()
            })
            .unwrap_or_default()
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn sample_count(&self) -> usize {
        self.buffer.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn stop(mut self) -> (Vec<f32>, u32, Option<String>) {
        drop(self.stream.take());
        // Give the audio backend a moment to finish its final callback.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let samples = self.snapshot();
        let error = self.take_error();
        (samples, self.sample_rate, error)
    }
}

fn make_error_callback(
    error: Arc<Mutex<Option<String>>>,
    device_name: String,
) -> impl FnMut(cpal::Error) + Send + 'static {
    move |err| {
        eprintln!("VoxKey: audio stream error ({device_name}): {err}");
        if let Ok(mut guard) = error.lock()
            && guard.is_none()
        {
            *guard = Some(err.to_string());
        }
    }
}

fn push_mono<T, F>(
    data: &[T],
    channels: usize,
    buffer: &Arc<Mutex<Vec<f32>>>,
    level: &Arc<AtomicU32>,
    to_float: F,
) where
    T: Copy,
    F: Fn(T) -> f32,
{
    if data.is_empty() || channels == 0 {
        return;
    }
    let mut mono = Vec::with_capacity(data.len() / channels);
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().map(|&sample| to_float(sample)).sum();
        mono.push(sum / frame.len() as f32);
    }
    if mono.is_empty() {
        return;
    }

    let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
    level.store(rms.min(1.0).to_bits(), Ordering::Relaxed);

    if let Ok(mut guard) = buffer.lock() {
        guard.extend_from_slice(&mono);
        // Safety valve: a stuck key should not consume unbounded memory.
        let max_samples = 48_000 * 180;
        if guard.len() > max_samples {
            let excess = guard.len() - max_samples;
            guard.drain(0..excess);
        }
    }
}

/// Linear interpolation resampler. Speech recognition input is always 16 kHz.
pub fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return samples.to_vec();
    }
    if samples.is_empty() {
        return Vec::new();
    }

    let source = samples.len() as f64;
    let out_len = ((source * to_rate as f64 / from_rate as f64).round() as usize).max(1);
    let step = from_rate as f64 / to_rate as f64;
    let mut output = Vec::with_capacity(out_len);

    for index in 0..out_len {
        let position = index as f64 * step;
        let left = position.floor() as usize;
        let fraction = (position - left as f64) as f32;
        let a = samples.get(left).copied().unwrap_or(0.0);
        let b = samples.get(left + 1).copied().unwrap_or(a);
        output.push(a + (b - a) * fraction);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_length_is_reasonable() {
        let input = vec![0.0f32; 48_000];
        let output = resample_linear(&input, 48_000, 16_000);
        assert_eq!(output.len(), 16_000);
    }

    #[test]
    fn resample_preserves_dc() {
        let input = vec![0.25f32; 100];
        let output = resample_linear(&input, 44_100, 16_000);
        assert!(output.iter().all(|s| (*s - 0.25).abs() < 1e-5));
    }
}
