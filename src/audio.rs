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
//! Provides three output layers over the pure procedural `DspEngine`:
//! 1. Cross-platform real-time audio via `cpal` (CoreAudio on macOS, WASAPI on Windows,
//!    ALSA on Linux/FreeBSD — zero allocations on callback thread).
//! 2. Automatic native pipe fallback for OpenBSD (`aucat` / `sndio`), static `musl` Linux
//!    (`pw-cat` / `pacat` / `aplay`), or FreeBSD/headless environments.
//! 3. Explicit Unix pipe mode (`--stdout-pcm`) writing raw interleaved stereo `f32le`
//!    PCM frames directly to `stdout`.

use std::io::{self, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(feature = "cpal")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
#[cfg(feature = "cpal")]
use cpal::{SampleFormat, Stream};

use crate::dsp::{DspEngine, SharedAudioBridge};

/// Holds the active audio output stream or background fallback worker.
pub struct AudioHandle {
    #[cfg(feature = "cpal")]
    _stream: Option<Stream>,
    running: Arc<AtomicBool>,
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Start the real-time audio backend.
/// Tries native `cpal` first (when compiled with the default `cpal` feature);
/// if unavailable (e.g. OpenBSD `sndio`, static `musl` builds, or headless SSH),
/// automatically tries a native system audio pipe (`aucat`, `pacat`, `aplay`) or
/// runs a silent timing loop so the visual waveform and lock-free queue still flow.
pub fn start_cpal_audio(bridge: Arc<SharedAudioBridge>, seed: u64) -> AudioHandle {
    let running = Arc::new(AtomicBool::new(true));

    #[cfg(feature = "cpal")]
    if let Some(stream) = try_init_cpal(Arc::clone(&bridge), seed) {
        return AudioHandle {
            _stream: Some(stream),
            running,
        };
    }

    let run_flag = Arc::clone(&running);
    thread::spawn(move || {
        let mut engine = Box::new(DspEngine::new(48_000.0, seed));
        let mut child_pipe = try_spawn_system_audio_pipe();
        let block_frames = 512usize;
        let sleep_dur = Duration::from_micros((block_frames as u64 * 1_000_000) / 48_000);
        let mut pcm_s16_buf = [0u8; 512 * 4];
        let mut next_tick = Instant::now();

        while run_flag.load(Ordering::Relaxed) {
            engine.poll_commands(&bridge);
            for i in 0..block_frames {
                let (l, r) = engine.next_stereo_frame(&bridge);
                let li = (l.clamp(-1.0, 1.0) * 32767.0) as i16;
                let ri = (r.clamp(-1.0, 1.0) * 32767.0) as i16;
                let off = i * 4;
                pcm_s16_buf[off..off + 2].copy_from_slice(&li.to_le_bytes());
                pcm_s16_buf[off + 2..off + 4].copy_from_slice(&ri.to_le_bytes());
            }

            if let Some(ref mut child) = child_pipe {
                let write_ok = child
                    .stdin
                    .as_mut()
                    .map(|stdin| stdin.write_all(&pcm_s16_buf).is_ok())
                    .unwrap_or(false);
                if !write_ok {
                    let _ = child.kill();
                    let _ = child.wait();
                    child_pipe = None;
                }
            }

            next_tick += sleep_dur;
            let now = Instant::now();
            if next_tick > now {
                thread::sleep(next_tick - now);
            } else {
                next_tick = now;
            }
        }

        if let Some(mut child) = child_pipe {
            let _ = child.kill();
            let _ = child.wait();
        }
    });

    AudioHandle {
        #[cfg(feature = "cpal")]
        _stream: None,
        running,
    }
}

/// Attempt to spawn a zero-configuration system audio player accepting raw 48kHz stereo `s16le` PCM on `stdin`.
/// Covers OpenBSD (`aucat`), PulseAudio/PipeWire (`pacat`), and ALSA (`aplay`).
fn try_spawn_system_audio_pipe() -> Option<Child> {
    let candidates: &[(&str, &[&str])] = &[
        // OpenBSD native sndio tool
        ("aucat", &["-f", "default", "-e", "s16le", "-r", "48000", "-c", "0:1", "-i", "-"]),
        // PulseAudio / PipeWire-Pulse stream player
        ("pacat", &["--playback", "--format=s16le", "--rate=48000", "--channels=2", "--latency-msec=40"]),
        // ALSA native player (useful on static musl Linux & Raspberry Pi)
        ("aplay", &["-q", "-f", "S16_LE", "-r", "48000", "-c", "2", "-"]),
    ];

    for &(bin, args) in candidates {
        if let Ok(child) = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            return Some(child);
        }
    }
    None
}

#[cfg(feature = "cpal")]
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
