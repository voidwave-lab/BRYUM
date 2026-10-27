// SPDX-License-Identifier: GPL-3.0-or-later
//
// BRYUM — Generative Ambient Sound Garden in the Terminal
// Copyright (C) 2026 Voidwave Lab
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Audio Output Backend (`audio.rs`)
//!
//! Provides two output paths over the pure procedural `DspEngine`:
//! 1. Cross-platform real-time audio via `cpal` (zero allocations on callback thread).
//! 2. Future-proof Unix pipe mode (`--stdout-pcm`) writing raw interleaved stereo `f32le`
//!    PCM frames directly to `stdout`.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};

use crate::dsp::{DspEngine, SharedAudioBridge};

/// Holds the active audio output stream or background fallback worker.
pub struct AudioHandle {
    _stream: Option<Stream>,
    running: Arc<AtomicBool>,
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Start the native `cpal` real-time audio stream.
/// If the system has no available audio device (e.g. headless SSH), falls back to a
/// silent background DSP consumer so the visual waveform and lock-free queue still flow.
pub fn start_cpal_audio(bridge: Arc<SharedAudioBridge>, seed: u64) -> AudioHandle {
    let running = Arc::new(AtomicBool::new(true));

    if let Some(stream) = try_init_cpal(Arc::clone(&bridge), seed) {
        return AudioHandle {
            _stream: Some(stream),
            running,
        };
    }

    // Headless fallback thread so waveform & queue continue functioning without audio hardware
    let run_flag = Arc::clone(&running);
    thread::spawn(move || {
        let mut engine = Box::new(DspEngine::new(48_000.0, seed));
        let block_frames = 512usize;
        let sleep_dur = Duration::from_micros((block_frames as u64 * 1_000_000) / 48_000);
        while run_flag.load(Ordering::Relaxed) {
            engine.poll_commands(&bridge);
            for _ in 0..block_frames {
                let _ = engine.next_stereo_frame(&bridge);
            }
            thread::sleep(sleep_dur);
        }
    });

    AudioHandle {
        _stream: None,
        running,
    }
}

fn try_init_cpal(bridge: Arc<SharedAudioBridge>, seed: u64) -> Option<Stream> {
    let host = cpal::default_host();
    let device = host.default_output_device()?;
    let supported_config = device.default_output_config().ok()?;
    let sample_format = supported_config.sample_format();
    let config: cpal::StreamConfig = supported_config.into();
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;

    // Pre-allocate the DSP engine on the heap before entering the real-time thread
    let mut engine = Box::new(DspEngine::new(sample_rate, seed));
    let err_fn = move |_err| {
        // Never write to stderr while TUI raw mode is active
    };

    let stream = match sample_format {
        SampleFormat::F32 => device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    engine.poll_commands(&bridge);
                    for frame in data.chunks_mut(channels.max(1)) {
                        let (l, r) = engine.next_stereo_frame(&bridge);
                        if frame.len() == 1 {
                            frame[0] = 0.5 * (l + r);
                        } else {
                            frame[0] = l;
                            frame[1] = r;
                            for ch in &mut frame[2..] {
                                *ch = 0.0;
                            }
                        }
                    }
                },
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::I16 => device
            .build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    engine.poll_commands(&bridge);
                    for frame in data.chunks_mut(channels.max(1)) {
                        let (l, r) = engine.next_stereo_frame(&bridge);
                        if frame.len() == 1 {
                            let m = (0.5 * (l + r) * 32767.0) as i16;
                            frame[0] = m;
                        } else {
                            frame[0] = (l * 32767.0) as i16;
                            frame[1] = (r * 32767.0) as i16;
                            for ch in &mut frame[2..] {
                                *ch = 0;
                            }
                        }
                    }
                },
                err_fn,
                None,
            )
            .ok()?,
        SampleFormat::U16 => device
            .build_output_stream(
                &config,
                move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                    engine.poll_commands(&bridge);
                    for frame in data.chunks_mut(channels.max(1)) {
                        let (l, r) = engine.next_stereo_frame(&bridge);
                        let to_u16 = |x: f32| ((x * 0.5 + 0.5) * 65535.0) as u16;
                        if frame.len() == 1 {
                            frame[0] = to_u16(0.5 * (l + r));
                        } else {
                            frame[0] = to_u16(l);
                            frame[1] = to_u16(r);
                            for ch in &mut frame[2..] {
                                *ch = 32768;
                            }
                        }
                    }
                },
                err_fn,
                None,
            )
            .ok()?,
        _ => return None,
    };

    stream.play().ok()?;
    Some(stream)
}

/// Run the `--stdout-pcm` headless Unix pipe mode.
/// Simulates the living garden in real time while streaming raw `f32le` stereo PCM
/// (48,000 Hz, 2 channels) directly to `stdout`.
pub fn run_stdout_pcm_stream(seed_code: String, seed: u64) -> io::Result<()> {
    use crate::garden::Garden;

    const SAMPLE_RATE: u32 = 48_000;
    const BLOCK_FRAMES: usize = 800; // 60 blocks per second (16.67ms per block)

    let bridge = Arc::new(SharedAudioBridge::new());
    let mut garden = Garden::new(seed_code);
    garden.sync_drones_and_streams(&bridge);

    let mut engine = Box::new(DspEngine::new(SAMPLE_RATE as f32, seed));
    let stdout = io::stdout();
    let mut handle = io::BufWriter::with_capacity(BLOCK_FRAMES * 8, stdout.lock());
    let mut byte_buf = [0u8; BLOCK_FRAMES * 8];

    let dt = (BLOCK_FRAMES as f32) / (SAMPLE_RATE as f32);
    let block_dur = Duration::from_secs_f64(dt as f64);
    let mut next_deadline = Instant::now();

    loop {
        garden.tick(dt, &bridge);
        engine.poll_commands(&bridge);

        for i in 0..BLOCK_FRAMES {
            let (l, r) = engine.next_stereo_frame(&bridge);
            let offset = i * 8;
            byte_buf[offset..offset + 4].copy_from_slice(&l.to_le_bytes());
            byte_buf[offset + 4..offset + 8].copy_from_slice(&r.to_le_bytes());
        }

        if let Err(e) = handle.write_all(&byte_buf) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e);
        }
        handle.flush()?;

        next_deadline += block_dur;
        let now = Instant::now();
        if next_deadline > now {
            thread::sleep(next_deadline - now);
        } else {
            next_deadline = now;
        }
    }
}
