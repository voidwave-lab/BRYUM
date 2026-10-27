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

//! Pure Procedural DSP Engine (`dsp.rs`)
//!
//! Zero external dependencies (strictly Rust `std`/`core`).
//! Zero heap allocations on the audio thread: all voice pools, delay lines,
//! and lock-free ringbuffers use fixed-size inline arrays.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Number of drone voices in the pool.
pub const MAX_DRONES: usize = 4;
/// Number of resonator voices (Leaf Karplus-Strong + Star FM droplets).
pub const MAX_RESONATORS: usize = 12;
/// Maximum Karplus-Strong delay buffer length per resonator voice.
const KS_BUFFER_LEN: usize = 2048;
/// Lock-free command ring buffer capacity (must be power of two).
const CMD_QUEUE_CAP: usize = 256;
/// Number of samples exposed to the UI for the breathing waveform display.
pub const WAVEFORM_BINS: usize = 64;

// FDN Reverb delay line lengths (mutually prime lengths in samples around 48kHz,
// padded for LFO modulation excursion).
const FDN_BUF_LEN: usize = 8192;
const FDN_BASE_DELAYS: [usize; 4] = [3557, 4423, 5471, 6829];
const AP_BUF_LEN: usize = 2048;
const AP_DELAYS_L: [usize; 2] = [347, 1139];
const AP_DELAYS_R: [usize; 2] = [389, 1217];

/// The four harmonic/color Seasons of BRYUM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    /// Lydian mode — warm gold & sunlit moss.
    Dawn,
    /// Dorian mode — moss green & cyan mist.
    Mist,
    /// Minor Pentatonic — amber & twilight violet.
    Dusk,
    /// Hirajoshi pentatonic — deep indigo & starlight.
    Night,
}

impl Season {
    pub const fn next(self) -> Self {
        match self {
            Self::Dawn => Self::Mist,
            Self::Mist => Self::Dusk,
            Self::Dusk => Self::Night,
            Self::Night => Self::Dawn,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Dawn => "Dawn",
            Self::Mist => "Mist",
            Self::Dusk => "Dusk",
            Self::Night => "Night",
        }
    }

    /// Semitone offsets from the root note for each Season's consonant mode.
    pub const fn scale_intervals(self) -> &'static [i32] {
        match self {
            // Lydian: 1, 2, 3, #4, 5, 6, 7
            Self::Dawn => &[0, 2, 4, 6, 7, 9, 11],
            // Dorian: 1, 2, b3, 4, 5, 6, b7
            Self::Mist => &[0, 2, 3, 5, 7, 9, 10],
            // Minor Pentatonic: 1, b3, 4, 5, b7
            Self::Dusk => &[0, 3, 5, 7, 10],
            // Hirajoshi: 1, 2, b3, 5, b6
            Self::Night => &[0, 2, 3, 7, 8],
        }
    }

    /// Base MIDI root note for each season to subtly shift the harmonic gravity.
    pub const fn root_midi(self) -> i32 {
        match self {
            Self::Dawn => 38,  // D2
            Self::Mist => 36,  // C2
            Self::Dusk => 39,  // Eb2
            Self::Night => 35, // B1
        }
    }
}

/// Built-in deterministic Xorshift64 PRNG seeded by the 6-character Garden Seed.
#[derive(Clone, Copy, Debug)]
pub struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    pub const fn new(seed: u64) -> Self {
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self { state }
    }

    /// Create a PRNG from a 6-character alphanumeric Garden Seed (e.g., `"KORES7"`).
    pub fn from_garden_code(code: &str) -> Self {
        let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
        for b in code.bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0100_0000_01B3);
        }
        Self::new(hash)
    }

    /// Format a 64-bit seed into a deterministic 6-character Garden Seed string.
    pub fn format_garden_code(seed: u64) -> String {
        const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = Self::new(seed);
        let mut out = String::with_capacity(6);
        for _ in 0..6 {
            let idx = (rng.next_u64() as usize) % ALPHABET.len();
            out.push(ALPHABET[idx] as char);
        }
        out
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform float in `[0.0, 1.0)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as u32 as f32) * (1.0 / (1u32 << 24) as f32)
    }

    /// Bipolar float in `[-1.0, 1.0)`.
    #[inline]
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

/// Quantizes a normalized vertical coordinate `y_norm` in `[0.0, 1.0]` into a consonant
/// frequency (Hz) for the given `Season` and octave span.
pub fn quantize_pitch(season: Season, y_norm: f32, base_octave_offset: i32, octave_span: i32) -> f32 {
    let intervals = season.scale_intervals();
    let total_degrees = (intervals.len() as i32) * octave_span;
    let clamped_y = y_norm.clamp(0.0, 0.9999);
    let degree_idx = (clamped_y * total_degrees as f32) as i32;
    let octave = degree_idx / (intervals.len() as i32);
    let step = (degree_idx % (intervals.len() as i32)) as usize;

    let midi = season.root_midi() + base_octave_offset * 12 + octave * 12 + intervals[step];
    midi_to_hz(midi as f32)
}

#[inline]
pub fn midi_to_hz(midi: f32) -> f32 {
    440.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
}

/// 2-pole Chamberlain State-Variable Filter (SVF).
#[derive(Clone, Copy, Debug, Default)]
pub struct ChamberlainSvf {
    low: f32,
    band: f32,
    high: f32,
    notch: f32,
}

impl ChamberlainSvf {
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Process a single sample with cutoff `freq_hz` and damping `q_damp` (lower = more resonant, e.g. 0.15..1.4).
    #[inline]
    pub fn process(&mut self, input: f32, freq_hz: f32, q_damp: f32, sample_rate: f32) -> (f32, f32, f32) {
        let safe_freq = freq_hz.clamp(20.0, sample_rate * 0.24);
        let f = (2.0 * ( std::f32::consts::PI * safe_freq / sample_rate ).sin()).clamp(0.001, 0.95);
        let q = q_damp.clamp(0.08, 1.8);

        self.low += f * self.band;
        self.high = input - self.low - q * self.band;
        self.band += f * self.high;
        self.notch = self.high + self.low;

        // Guard against denormals or numerical blowup
        if !self.low.is_finite() || !self.band.is_finite() {
            self.reset();
        } else {
            self.low = self.low.clamp(-8.0, 8.0);
            self.band = self.band.clamp(-8.0, 8.0);
        }

        (self.low, self.band, self.high)
    }
}

/// Drone Voice (Stone): Detuned dual-saw/triangle oscillator + sub-harmonic with
/// slow internal LFO modulating a 2-pole Chamberlain SVF.
#[derive(Clone, Copy, Debug)]
pub struct DroneVoice {
    pub active: bool,
    pub target_freq: f32,
    pub current_freq: f32,
    pub target_pan: f32,
    pub current_pan: f32,
    pub stream_warmth: f32,
    gain: f32,
    phase_a: f32,
    phase_b: f32,
    phase_sub: f32,
    lfo_phase: f32,
    lfo_rate: f32,
    svf: ChamberlainSvf,
}

impl DroneVoice {
    pub const fn new(index: usize) -> Self {
        Self {
            active: false,
            target_freq: 110.0,
            current_freq: 110.0,
            target_pan: 0.0,
            current_pan: 0.0,
            stream_warmth: 0.0,
            gain: 0.0,
            phase_a: 0.0,
            phase_b: 0.37,
            phase_sub: 0.0,
            lfo_phase: (index as f32) * 0.25,
            lfo_rate: 0.07 + (index as f32) * 0.031,
            svf: ChamberlainSvf {
                low: 0.0,
                band: 0.0,
                high: 0.0,
                notch: 0.0,
            },
        }
    }

    #[inline]
    pub fn process(&mut self, sample_rate: f32, wind_speed: f32, mist_depth: f32) -> (f32, f32) {
        let target_gain = if self.active { 0.22 } else { 0.0 };
        let slew = if self.active { 0.00015 } else { 0.00008 };
        self.gain += (target_gain - self.gain) * slew;

        if self.gain < 1.0e-5 && !self.active {
            self.gain = 0.0;
            return (0.0, 0.0);
        }

        // Smooth glide for frequency & stereo pan
        self.current_freq += (self.target_freq - self.current_freq) * 0.0005;
        self.current_pan += (self.target_pan - self.current_pan) * 0.001;

        let inv_sr = 1.0 / sample_rate;

        // Slow internal LFO
        let lfo_inc = (self.lfo_rate * (0.6 + wind_speed * 1.2)) * inv_sr;
        self.lfo_phase = (self.lfo_phase + lfo_inc).fract();
        let lfo = (self.lfo_phase * TAU).sin();

        // Detuned dual oscillator frequencies
        let detune = 1.0035 + 0.0015 * lfo;
        let freq_a = self.current_freq;
        let freq_b = self.current_freq * detune;

        self.phase_a = (self.phase_a + freq_a * inv_sr).fract();
        self.phase_b = (self.phase_b + freq_b * inv_sr).fract();
        self.phase_sub = (self.phase_sub + (freq_a * 0.5) * inv_sr).fract();

        // Oscillator A: warm saw + triangle blend
        let saw_a = 2.0 * self.phase_a - 1.0;
        let tri_a = 1.0 - 4.0 * (self.phase_a - 0.5).abs();
        let osc_a = 0.55 * tri_a + 0.45 * saw_a;

        // Oscillator B: detuned triangle + saw blend
        let saw_b = 2.0 * self.phase_b - 1.0;
        let tri_b = 1.0 - 4.0 * (self.phase_b - 0.5).abs();
        let osc_b = 0.65 * tri_b + 0.35 * saw_b;

        // Sub-sine for warm grounding
        let sub = (self.phase_sub * TAU).sin() * 0.45;

        let raw = (osc_a + osc_b + sub) * 0.45;

        // Modulate Chamberlain SVF cutoff with internal LFO, Mist warmth, and nearby Stream proximity
        let warmth_boost = 1.0 + self.stream_warmth * 2.8 + mist_depth * 0.9;
        let cutoff = (self.current_freq * (2.2 + 1.1 * lfo) * warmth_boost).clamp(70.0, 2600.0);
        let (low, band, _) = self.svf.process(raw, cutoff, 0.55, sample_rate);
        let filtered = (low * 0.85 + band * 0.15) * self.gain;

        // Equal-power stereo panning
        let pan_norm = (self.current_pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
        let angle = pan_norm * std::f32::consts::FRAC_PI_2;
        (filtered * angle.cos(), filtered * angle.sin())
    }
}

/// Synthesis mode for a Resonator Voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResonatorKind {
    /// Extended Karplus-Strong plucked string / wooden chime (Leaf element).
    KarplusStrong,
    /// 2-operator crystalline FM droplet with long exponential decay (Star / Spore element).
    FmDroplet,
}

/// Resonator Voice (12x polyphonic pool): handles both Karplus-Strong plucks and 2-op FM droplets.
#[derive(Clone)]
pub struct ResonatorVoice {
    pub active: bool,
    pub kind: ResonatorKind,
    pub age: u64,
    freq: f32,
    pan: f32,
    env: f32,
    env_decay: f32,
    mod_env: f32,
    mod_decay: f32,
    // FM state
    carrier_phase: f32,
    mod_phase: f32,
    mod_ratio: f32,
    mod_index: f32,
    // Extended Karplus-Strong state
    ks_buf: [f32; KS_BUFFER_LEN],
    ks_len: usize,
    ks_pos: usize,
    ks_lp_state: f32,
    ks_ap_state: f32,
    ks_ap_prev: f32,
    ks_feedback: f32,
    body_svf: ChamberlainSvf,
}

impl ResonatorVoice {
    pub const fn new() -> Self {
        Self {
            active: false,
            kind: ResonatorKind::KarplusStrong,
            age: 0,
            freq: 440.0,
            pan: 0.0,
            env: 0.0,
            env_decay: 0.9995,
            mod_env: 0.0,
            mod_decay: 0.999,
            carrier_phase: 0.0,
            mod_phase: 0.0,
            mod_ratio: 2.0,
            mod_index: 1.5,
            ks_buf: [0.0; KS_BUFFER_LEN],
            ks_len: 128,
            ks_pos: 0,
            ks_lp_state: 0.0,
            ks_ap_state: 0.0,
            ks_ap_prev: 0.0,
            ks_feedback: 0.994,
            body_svf: ChamberlainSvf {
                low: 0.0,
                band: 0.0,
                high: 0.0,
                notch: 0.0,
            },
        }
    }

    /// Trigger an extended Karplus-Strong pluck (Leaf / Chime).
    pub fn trigger_pluck(
        &mut self,
        freq: f32,
        pan: f32,
        velocity: f32,
        sample_rate: f32,
        rng: &mut Xorshift64,
        age: u64,
    ) {
        self.active = true;
        self.kind = ResonatorKind::KarplusStrong;
        self.age = age;
        self.freq = freq.clamp(65.0, 2400.0);
        self.pan = pan.clamp(-1.0, 1.0);
        self.env = velocity.clamp(0.1, 1.0);
        // About 3.5 seconds envelope guard
        self.env_decay = (-1.0 / (sample_rate * 3.8)).exp();
        self.ks_len = ((sample_rate / self.freq).round() as usize).clamp(16, KS_BUFFER_LEN - 1);
        self.ks_pos = 0;
        self.ks_lp_state = 0.0;
        self.ks_ap_state = 0.0;
        self.ks_ap_prev = 0.0;
        self.ks_feedback = 0.9955;
        self.body_svf.reset();

        // Fill the excitation delay line with warm low-passed noise + fundamental sine burst
        let mut smooth = 0.0_f32;
        for i in 0..self.ks_len {
            let phase = (i as f32) / (self.ks_len as f32);
            let window = (phase * std::f32::consts::PI).sin();
            let white = rng.next_bipolar();
            smooth = smooth * 0.68 + white * 0.32;
            let fundamental = (phase * TAU).sin() * 0.35;
            self.ks_buf[i] = (smooth * 0.75 + fundamental) * window * self.env;
        }
    }

    /// Trigger a 2-operator crystalline FM droplet (Star / Spore).
    pub fn trigger_fm_droplet(
        &mut self,
        freq: f32,
        pan: f32,
        velocity: f32,
        sample_rate: f32,
        rng: &mut Xorshift64,
        age: u64,
    ) {
        self.active = true;
        self.kind = ResonatorKind::FmDroplet;
        self.age = age;
        self.freq = freq.clamp(220.0, 4200.0);
        self.pan = pan.clamp(-1.0, 1.0);
        self.env = velocity.clamp(0.1, 1.0) * 0.42;
        // Long crystalline tail (2.8 to 4.5 seconds)
        let decay_secs = 2.8 + rng.next_f32() * 1.8;
        self.env_decay = (-1.0 / (sample_rate * decay_secs)).exp();
        self.mod_env = 1.0;
        self.mod_decay = (-1.0 / (sample_rate * (0.25 + rng.next_f32() * 0.45))).exp();
        self.carrier_phase = 0.0;
        self.mod_phase = 0.0;
        // Consonant crystalline harmonic ratios
        const RATIOS: [f32; 4] = [2.0, 3.0, 4.0, 1.5];
        self.mod_ratio = RATIOS[(rng.next_u64() as usize) % RATIOS.len()];
        self.mod_index = 1.1 + rng.next_f32() * 1.6;
    }

    #[inline]
    pub fn process(&mut self, sample_rate: f32) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }

        let sig = match self.kind {
            ResonatorKind::KarplusStrong => {
                let cur = self.ks_buf[self.ks_pos];
                let next_pos = if self.ks_pos + 1 >= self.ks_len {
                    0
                } else {
                    self.ks_pos + 1
                };
                let next = self.ks_buf[next_pos];

                // One-zero + one-pole loop damping filter
                let avg = 0.5 * (cur + next);
                self.ks_lp_state = 0.72 * avg + 0.28 * self.ks_lp_state;

                // Allpass dispersion filter for wood/bell inharmonic warmth
                let ap_coeff = 0.35;
                let ap_out = -ap_coeff * self.ks_lp_state + self.ks_ap_prev + ap_coeff * self.ks_ap_state;
                self.ks_ap_prev = self.ks_lp_state;
                self.ks_ap_state = ap_out;

                self.ks_buf[self.ks_pos] = ap_out * self.ks_feedback;
                self.ks_pos = next_pos;

                self.env *= self.env_decay;
                let (low, band, _) = self.body_svf.process(cur, self.freq * 2.5, 0.45, sample_rate);
                (cur * 0.65 + low * 0.25 + band * 0.15) * 0.45
            }
            ResonatorKind::FmDroplet => {
                let inv_sr = 1.0 / sample_rate;
                let mod_freq = self.freq * self.mod_ratio;
                self.mod_phase = (self.mod_phase + mod_freq * inv_sr).fract();
                let mod_sig = (self.mod_phase * TAU).sin() * self.mod_index * self.mod_env;

                self.carrier_phase = (self.carrier_phase + self.freq * inv_sr).fract();
                let carrier = (self.carrier_phase * TAU + mod_sig).sin();

                self.env *= self.env_decay;
                self.mod_env *= self.mod_decay;
                carrier * self.env
            }
        };

        if self.env < 1.0e-4 {
            self.active = false;
            return (0.0, 0.0);
        }

        let pan_norm = (self.pan + 1.0) * 0.5;
        let angle = pan_norm * std::f32::consts::FRAC_PI_2;
        (sig * angle.cos(), sig * angle.sin())
    }
}

/// Paul Kellet's Pink Noise Generator + Dual Wandering Stereo Bandpass SVFs (Stream Element).
#[derive(Clone, Copy, Debug)]
pub struct StreamGenerator {
    b0: f32,
    b1: f32,
    b2: f32,
    b3: f32,
    b4: f32,
    b5: f32,
    b6: f32,
    svf_l: ChamberlainSvf,
    svf_r: ChamberlainSvf,
    lfo_phase_l: f32,
    lfo_phase_r: f32,
    pub target_level: f32,
    current_level: f32,
}

impl StreamGenerator {
    pub const fn new() -> Self {
        Self {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            b3: 0.0,
            b4: 0.0,
            b5: 0.0,
            b6: 0.0,
            svf_l: ChamberlainSvf {
                low: 0.0,
                band: 0.0,
                high: 0.0,
                notch: 0.0,
            },
            svf_r: ChamberlainSvf {
                low: 0.0,
                band: 0.0,
                high: 0.0,
                notch: 0.0,
            },
            lfo_phase_l: 0.1,
            lfo_phase_r: 0.63,
            target_level: 0.0,
            current_level: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, sample_rate: f32, wind_speed: f32, rng: &mut Xorshift64) -> (f32, f32) {
        self.current_level += (self.target_level - self.current_level) * 0.0002;
        if self.current_level < 1.0e-5 {
            return (0.0, 0.0);
        }

        // Paul Kellet's refined pink noise filter
        let white = rng.next_bipolar();
        self.b0 = 0.99886 * self.b0 + white * 0.0555179;
        self.b1 = 0.99332 * self.b1 + white * 0.0750759;
        self.b2 = 0.96900 * self.b2 + white * 0.1538520;
        self.b3 = 0.86650 * self.b3 + white * 0.3104856;
        self.b4 = 0.55000 * self.b4 + white * 0.5329522;
        self.b5 = -0.7616 * self.b5 - white * 0.0168980;
        let pink = (self.b0 + self.b1 + self.b2 + self.b3 + self.b4 + self.b5 + self.b6 + white * 0.5362) * 0.04;
        self.b6 = white * 0.115926;

        let inv_sr = 1.0 / sample_rate;
        let rate_scale = 0.4 + wind_speed * 1.4;
        self.lfo_phase_l = (self.lfo_phase_l + 0.11 * rate_scale * inv_sr).fract();
        self.lfo_phase_r = (self.lfo_phase_r + 0.147 * rate_scale * inv_sr).fract();

        let lfo_l = (self.lfo_phase_l * TAU).sin();
        let lfo_r = (self.lfo_phase_r * TAU).sin();

        let cutoff_l = 340.0 + 240.0 * lfo_l + wind_speed * 260.0;
        let cutoff_r = 420.0 + 280.0 * lfo_r + wind_speed * 260.0;

        let (low_l, band_l, _) = self.svf_l.process(pink, cutoff_l, 0.38, sample_rate);
        let (low_r, band_r, _) = self.svf_r.process(pink, cutoff_r, 0.38, sample_rate);

        let out_l = (band_l * 0.75 + low_l * 0.25) * self.current_level * 0.38;
        let out_r = (band_r * 0.75 + low_r * 0.25) * self.current_level * 0.38;
        (out_l, out_r)
    }
}

/// Schroeder Allpass Diffuser used inside the stereo reverb.
#[derive(Clone)]
struct AllpassDiffuser {
    buf: [f32; AP_BUF_LEN],
    len: usize,
    pos: usize,
    feedback: f32,
}

impl AllpassDiffuser {
    const fn new(len: usize, feedback: f32) -> Self {
        Self {
            buf: [0.0; AP_BUF_LEN],
            len,
            pos: 0,
            feedback,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buf[self.pos];
        let v = input - delayed * self.feedback;
        self.buf[self.pos] = v;
        self.pos += 1;
        if self.pos >= self.len {
            self.pos = 0;
        }
        delayed + v * self.feedback
    }
}

/// 4x4 Hadamard Feedback Delay Network (FDN) Stereo Reverb with modulated delay lines
/// and 6–15s lush ambient decay.
#[derive(Clone)]
pub struct StereoReverb {
    ap_l: [AllpassDiffuser; 2],
    ap_r: [AllpassDiffuser; 2],
    fdn_bufs: [[f32; FDN_BUF_LEN]; 4],
    fdn_write: [usize; 4],
    fdn_lp: [f32; 4],
    lfo_phases: [f32; 4],
}

impl StereoReverb {
    pub const fn new() -> Self {
        Self {
            ap_l: [
                AllpassDiffuser::new(AP_DELAYS_L[0], 0.65),
                AllpassDiffuser::new(AP_DELAYS_L[1], 0.62),
            ],
            ap_r: [
                AllpassDiffuser::new(AP_DELAYS_R[0], 0.65),
                AllpassDiffuser::new(AP_DELAYS_R[1], 0.62),
            ],
            fdn_bufs: [[0.0; FDN_BUF_LEN]; 4],
            fdn_write: [0; 4],
            fdn_lp: [0.0; 4],
            lfo_phases: [0.0, 0.25, 0.5, 0.75],
        }
    }

    #[inline]
    pub fn process(&mut self, in_l: f32, in_r: f32, mist_depth: f32, sample_rate: f32) -> (f32, f32) {
        // Diffuse stereo inputs
        let ap0_l = self.ap_l[0].process(in_l);
        let diff_l = self.ap_l[1].process(ap0_l);
        let ap0_r = self.ap_r[0].process(in_r);
        let diff_r = self.ap_r[1].process(ap0_r);

        let inv_sr = 1.0 / sample_rate;
        let mut delayed = [0.0_f32; 4];

        // Read from modulated delay lines with linear interpolation
        const LFO_RATES: [f32; 4] = [0.23, 0.31, 0.41, 0.53];
        for i in 0..4 {
            self.lfo_phases[i] = (self.lfo_phases[i] + LFO_RATES[i] * inv_sr).fract();
            let mod_offset = 12.0 * (self.lfo_phases[i] * TAU).sin();
            let delay_samples = (FDN_BASE_DELAYS[i] as f32) + mod_offset;

            let read_f = (self.fdn_write[i] as f32) + (FDN_BUF_LEN as f32) - delay_samples;
            let idx0 = (read_f as usize) & (FDN_BUF_LEN - 1);
            let idx1 = (idx0 + 1) & (FDN_BUF_LEN - 1);
            let frac = read_f.fract();

            delayed[i] = self.fdn_bufs[i][idx0] * (1.0 - frac) + self.fdn_bufs[i][idx1] * frac;
        }

        // 4x4 Hadamard orthogonal mixing matrix (normalized by 0.5)
        let s0 = delayed[0] + delayed[1];
        let s1 = delayed[0] - delayed[1];
        let s2 = delayed[2] + delayed[3];
        let s3 = delayed[2] - delayed[3];

        let mixed = [
            0.5 * (s0 + s2),
            0.5 * (s1 + s3),
            0.5 * (s0 - s2),
            0.5 * (s1 - s3),
        ];

        // Feedback gain corresponding to 6s..15s RT60 decay based on mist_depth
        let feedback = 0.962 + mist_depth.clamp(0.0, 1.0) * 0.029; // up to 0.991
        let damp = 0.26 - mist_depth.clamp(0.0, 1.0) * 0.10;
        let inputs = [diff_l, diff_r, diff_l * 0.7 - diff_r * 0.3, diff_r * 0.7 - diff_l * 0.3];

        for i in 0..4 {
            self.fdn_lp[i] = mixed[i] * (1.0 - damp) + self.fdn_lp[i] * damp;
            let next_val = (inputs[i] * 0.4 + self.fdn_lp[i] * feedback).clamp(-4.0, 4.0);
            let w = self.fdn_write[i];
            self.fdn_bufs[i][w] = if next_val.is_finite() { next_val } else { 0.0 };
            self.fdn_write[i] = (w + 1) & (FDN_BUF_LEN - 1);
        }

        let wet_l = (delayed[0] + delayed[2]) * 0.45;
        let wet_r = (delayed[1] + delayed[3]) * 0.45;

        let wet_mix = 0.38 + mist_depth * 0.44;
        let dry_mix = 1.0 - wet_mix * 0.45;

        (
            in_l * dry_mix + wet_l * wet_mix,
            in_r * dry_mix + wet_r * wet_mix,
        )
    }
}

/// Commands sent lock-free from the simulation/UI thread to the real-time DSP thread.
#[derive(Clone, Copy, Debug)]
pub enum DspCommand {
    /// Update a Stone (Drone) slot `0..MAX_DRONES`.
    SetDrone {
        slot: usize,
        active: bool,
        freq: f32,
        pan: f32,
        stream_warmth: f32,
    },
    /// Trigger a Leaf / Chime Karplus-Strong plucked resonator.
    TriggerPluck {
        freq: f32,
        pan: f32,
        velocity: f32,
    },
    /// Trigger a Star / Spore 2-operator FM droplet.
    TriggerDroplet {
        freq: f32,
        pan: f32,
        velocity: f32,
    },
    /// Set target Stream pink-noise level (`0.0..1.0`).
    SetStreamLevel(f32),
    /// Set macro atmosphere controls (`wind_speed`, `mist_depth`).
    SetMacros {
        wind_speed: f32,
        mist_depth: f32,
    },
    /// Reseed the internal PRNG.
    Reseed(u64),
}

/// Lock-free Single-Producer Single-Consumer (SPSC) bridge between UI and Audio thread.
/// Contains zero heap allocations during runtime.
pub struct SharedAudioBridge {
    cmd_buf: [std::cell::UnsafeCell<DspCommand>; CMD_QUEUE_CAP],
    write_idx: AtomicUsize,
    read_idx: AtomicUsize,
    waveform_bins: [AtomicU32; WAVEFORM_BINS],
    waveform_cursor: AtomicUsize,
}

// Safety: SPSC ring buffer uses Acquire/Release atomic indices for `cmd_buf` access,
// and `waveform_bins` uses atomic `u32` bit-cast floats.
unsafe impl Sync for SharedAudioBridge {}
unsafe impl Send for SharedAudioBridge {}

impl SharedAudioBridge {
    pub fn new() -> Self {
        Self {
            cmd_buf: std::array::from_fn(|_| {
                std::cell::UnsafeCell::new(DspCommand::SetStreamLevel(0.0))
            }),
            write_idx: AtomicUsize::new(0),
            read_idx: AtomicUsize::new(0),
            waveform_bins: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            waveform_cursor: AtomicUsize::new(0),
        }
    }

    /// Push a command from the UI/simulation thread (non-blocking, zero allocation).
    #[inline]
    pub fn push_cmd(&self, cmd: DspCommand) -> bool {
        let w = self.write_idx.load(Ordering::Relaxed);
        let r = self.read_idx.load(Ordering::Acquire);
        let next_w = (w + 1) & (CMD_QUEUE_CAP - 1);
        if next_w == r {
            return false; // Queue full
        }
        unsafe {
            *self.cmd_buf[w].get() = cmd;
        }
        self.write_idx.store(next_w, Ordering::Release);
        true
    }

    /// Pop a command on the real-time audio thread (non-blocking, zero allocation).
    #[inline]
    pub fn pop_cmd(&self) -> Option<DspCommand> {
        let r = self.read_idx.load(Ordering::Relaxed);
        let w = self.write_idx.load(Ordering::Acquire);
        if r == w {
            return None;
        }
        let cmd = unsafe { *self.cmd_buf[r].get() };
        let next_r = (r + 1) & (CMD_QUEUE_CAP - 1);
        self.read_idx.store(next_r, Ordering::Release);
        Some(cmd)
    }

    /// Record an output audio amplitude sample into the lock-free circular waveform display buffer.
    #[inline]
    pub fn record_waveform_sample(&self, sample: f32) {
        let idx = self.waveform_cursor.fetch_add(1, Ordering::Relaxed) % WAVEFORM_BINS;
        self.waveform_bins[idx].store(sample.to_bits(), Ordering::Relaxed);
    }

    /// Read the recent waveform samples in chronological order for UI rendering.
    pub fn read_waveform(&self, out: &mut [f32; WAVEFORM_BINS]) {
        let cursor = self.waveform_cursor.load(Ordering::Relaxed);
        for (i, slot) in out.iter_mut().enumerate() {
            let idx = (cursor + i) % WAVEFORM_BINS;
            *slot = f32::from_bits(self.waveform_bins[idx].load(Ordering::Relaxed));
        }
    }
}

/// Complete Real-Time Procedural DSP Engine.
pub struct DspEngine {
    sample_rate: f32,
    rng: Xorshift64,
    drones: [DroneVoice; MAX_DRONES],
    resonators: [ResonatorVoice; MAX_RESONATORS],
    stream_gen: StreamGenerator,
    reverb: StereoReverb,
    wind_speed: f32,
    mist_depth: f32,
    voice_counter: u64,
    scope_decimator: usize,
    scope_accum: f32,
}

impl DspEngine {
    pub fn new(sample_rate: f32, seed: u64) -> Self {
        Self {
            sample_rate: sample_rate.max(8000.0),
            rng: Xorshift64::new(seed),
            drones: [
                DroneVoice::new(0),
                DroneVoice::new(1),
                DroneVoice::new(2),
                DroneVoice::new(3),
            ],
            resonators: std::array::from_fn(|_| ResonatorVoice::new()),
            stream_gen: StreamGenerator::new(),
            reverb: StereoReverb::new(),
            wind_speed: 0.45,
            mist_depth: 0.65,
            voice_counter: 0,
            scope_decimator: 0,
            scope_accum: 0.0,
        }
    }

    /// Allocate the least-recently-used or inactive resonator voice from the 12-voice pool.
    #[inline]
    fn allocate_resonator(&mut self) -> usize {
        for (i, v) in self.resonators.iter().enumerate() {
            if !v.active {
                return i;
            }
        }
        // Steal the oldest voice
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;
        for (i, v) in self.resonators.iter().enumerate() {
            if v.age < oldest_age {
                oldest_age = v.age;
                oldest_idx = i;
            }
        }
        oldest_idx
    }

    /// Drain pending lock-free commands from the UI thread.
    #[inline]
    pub fn poll_commands(&mut self, bridge: &SharedAudioBridge) {
        while let Some(cmd) = bridge.pop_cmd() {
            match cmd {
                DspCommand::SetDrone {
                    slot,
                    active,
                    freq,
                    pan,
                    stream_warmth,
                } => {
                    if slot < MAX_DRONES {
                        let d = &mut self.drones[slot];
                        if active && !d.active {
                            d.current_freq = freq;
                            d.current_pan = pan;
                        }
                        d.active = active;
                        d.target_freq = freq;
                        d.target_pan = pan;
                        d.stream_warmth = stream_warmth;
                    }
                }
                DspCommand::TriggerPluck {
                    freq,
                    pan,
                    velocity,
                } => {
                    self.voice_counter = self.voice_counter.wrapping_add(1);
                    let age = self.voice_counter;
                    let sr = self.sample_rate;
                    let idx = self.allocate_resonator();
                    self.resonators[idx].trigger_pluck(freq, pan, velocity, sr, &mut self.rng, age);
                }
                DspCommand::TriggerDroplet {
                    freq,
                    pan,
                    velocity,
                } => {
                    self.voice_counter = self.voice_counter.wrapping_add(1);
                    let age = self.voice_counter;
                    let sr = self.sample_rate;
                    let idx = self.allocate_resonator();
                    self.resonators[idx].trigger_fm_droplet(
                        freq,
                        pan,
                        velocity,
                        sr,
                        &mut self.rng,
                        age,
                    );
                }
                DspCommand::SetStreamLevel(lvl) => {
                    self.stream_gen.target_level = lvl.clamp(0.0, 1.0);
                }
                DspCommand::SetMacros {
                    wind_speed,
                    mist_depth,
                } => {
                    self.wind_speed = wind_speed.clamp(0.0, 1.0);
                    self.mist_depth = mist_depth.clamp(0.0, 1.0);
                }
                DspCommand::Reseed(seed) => {
                    self.rng = Xorshift64::new(seed);
                }
            }
        }
    }

    /// Generate a single stereo frame `(left, right)` in `[-1.0, 1.0]`.
    #[inline]
    pub fn next_stereo_frame(&mut self, bridge: &SharedAudioBridge) -> (f32, f32) {
        let mut dry_l = 0.0_f32;
        let mut dry_r = 0.0_f32;

        // 1. Sum 4x Drone voices
        for drone in &mut self.drones {
            let (l, r) = drone.process(self.sample_rate, self.wind_speed, self.mist_depth);
            dry_l += l;
            dry_r += r;
        }

        // 2. Sum 12x Resonator voices (Karplus-Strong plucks + FM droplets)
        for res in &mut self.resonators {
            let (l, r) = res.process(self.sample_rate);
            dry_l += l;
            dry_r += r;
        }

        // 3. Stream generator (Paul Kellet pink noise + wandering stereo bandpass SVFs)
        let (st_l, st_r) = self
            .stream_gen
            .process(self.sample_rate, self.wind_speed, &mut self.rng);
        dry_l += st_l;
        dry_r += st_r;

        // 4. Global Stereo Reverb
        let (wet_l, wet_r) = self
            .reverb
            .process(dry_l, dry_r, self.mist_depth, self.sample_rate);

        // Soft saturation limiting so output never clips harshly
        let out_l = soft_clip(wet_l);
        let out_r = soft_clip(wet_r);

        // Downsample and record into lock-free waveform scope buffer (~30-60 updates/sec)
        let mono = 0.5 * (out_l + out_r);
        self.scope_accum = self.scope_accum * 0.98 + mono * 0.02;
        self.scope_decimator += 1;
        if self.scope_decimator >= 750 {
            self.scope_decimator = 0;
            bridge.record_waveform_sample(self.scope_accum);
        }

        (out_l, out_r)
    }
}

/// Smooth Padé-like soft clipper in `[-1.0, 1.0]`.
#[inline]
fn soft_clip(x: f32) -> f32 {
    let c = x.clamp(-3.0, 3.0);
    c * (27.0 + c * c) / (27.0 + 9.0 * c * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xorshift_determinism_and_seed_code() {
        let mut rng1 = Xorshift64::from_garden_code("KORES7");
        let mut rng2 = Xorshift64::from_garden_code("KORES7");
        for _ in 0..32 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
        let code = Xorshift64::format_garden_code(123456789);
        assert_eq!(code.len(), 6);
    }

    #[test]
    fn test_dsp_engine_finite_audio_generation() {
        let bridge = SharedAudioBridge::new();
        let mut engine = DspEngine::new(48_000.0, 42);
        bridge.push_cmd(DspCommand::SetDrone {
            slot: 0,
            active: true,
            freq: 110.0,
            pan: -0.3,
            stream_warmth: 0.5,
        });
        bridge.push_cmd(DspCommand::TriggerPluck {
            freq: 440.0,
            pan: 0.2,
            velocity: 0.8,
        });
        bridge.push_cmd(DspCommand::TriggerDroplet {
            freq: 880.0,
            pan: -0.4,
            velocity: 0.7,
        });
        bridge.push_cmd(DspCommand::SetStreamLevel(0.5));
        engine.poll_commands(&bridge);

        let mut max_abs = 0.0_f32;
        for _ in 0..4096 {
            let (l, r) = engine.next_stereo_frame(&bridge);
            assert!(l.is_finite() && r.is_finite());
            assert!(l.abs() <= 1.05 && r.abs() <= 1.05);
            max_abs = max_abs.max(l.abs()).max(r.abs());
        }
        assert!(max_abs > 0.01);
    }
}

