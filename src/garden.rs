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

//! 2D Terrarium & Sound Garden Simulation (`garden.rs`)
//!
//! Zero external dependencies (strictly Rust `std`/`core` + `crate::dsp`).
//! Simulates planted nature elements, drifting sinusoidal wind currents,
//! expanding visual ripples, and lock-free DSP trigger synchronization.

use std::f32::consts::TAU;

use crate::dsp::{
    quantize_pitch, DspCommand, Season, SharedAudioBridge, Xorshift64, MAX_DRONES,
};

/// Logical width of the terrarium canvas coordinate space.
pub const WORLD_WIDTH: f32 = 100.0;
/// Logical height of the terrarium canvas coordinate space.
pub const WORLD_HEIGHT: f32 = 100.0;

/// The four organic element types a user can plant in the terrarium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementKind {
    /// Stone (Root/Drone): Warm, slow-evolving low-register foundation.
    Stone,
    /// Leaf (Chime/Resonator): Awakens with a plucked tone when brushed by a wind current.
    Leaf,
    /// Stream (Water/Flow): Soft water/wind texture that warms nearby Stones.
    Stream,
    /// Star (Spore/Shimmer): Emits rare, high-register crystalline droplets on its own rhythm.
    Star,
}

impl ElementKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stone => "Stone",
            Self::Leaf => "Leaf",
            Self::Stream => "Stream",
            Self::Star => "Star",
        }
    }
}

/// A single planted element inside the terrarium.
#[derive(Clone, Copy, Debug)]
pub struct PlantedElement {
    pub kind: ElementKind,
    /// Horizontal position in `[0.0, WORLD_WIDTH]`.
    pub x: f32,
    /// Vertical position in `[0.0, WORLD_HEIGHT]` (higher Y = higher pitch).
    pub y: f32,
    /// Cooldown timer (seconds) so wind waves don't re-trigger the same Leaf every frame.
    pub cooldown: f32,
    /// Glow / excitation envelope `[0.0, 1.0]` for visual feedback when triggered.
    pub glow: f32,
    /// Individual phase offset for gentle visual breathing/swaying.
    pub phase: f32,
}

/// An expanding visual ripple spawned when a Leaf is brushed by wind or a Star chimes.
#[derive(Clone, Copy, Debug)]
pub struct VisualRipple {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub max_radius: f32,
    /// Remaining envelope `[0.0, 1.0]` — fades RGB toward background as audio decays.
    pub life: f32,
    pub decay_rate: f32,
    pub kind: ElementKind,
}

/// A drifting sinusoidal wind current flowing across the terrarium.
#[derive(Clone, Copy, Debug)]
pub struct WindCurrent {
    /// Base vertical center in `[0.0, WORLD_HEIGHT]` or horizontal drift phase.
    pub center_x: f32,
    pub amplitude: f32,
    pub wavelength: f32,
    pub speed: f32,
    pub phase: f32,
}

impl WindCurrent {
    /// Evaluate the horizontal X position of this wind wave front at vertical coordinate `y` and time `t`.
    #[inline]
    pub fn wave_x_at(&self, y: f32, t: f32) -> f32 {
        let wave_offset = ((y / self.wavelength) * TAU + self.phase + t * self.speed * 1.7).sin()
            * self.amplitude;
        (self.center_x + wave_offset).rem_euclid(WORLD_WIDTH)
    }
}

/// Macro control selection for keyboard adjustment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroControl {
    WindSpeed,
    MistDepth,
    Abundance,
}

impl MacroControl {
    pub const fn next(self) -> Self {
        match self {
            Self::WindSpeed => Self::MistDepth,
            Self::MistDepth => Self::Abundance,
            Self::Abundance => Self::WindSpeed,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::WindSpeed => Self::Abundance,
            Self::MistDepth => Self::WindSpeed,
            Self::Abundance => Self::MistDepth,
        }
    }
}

/// Complete state of the BRYUM generative sound garden.
pub struct Garden {
    pub seed_code: String,
    pub rng: Xorshift64,
    pub season: Season,
    pub elements: Vec<PlantedElement>,
    pub ripples: Vec<VisualRipple>,
    pub wind_currents: [WindCurrent; 3],
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub selected_tool: ElementKind,
    pub selected_macro: MacroControl,
    /// Wind Speed macro in `[0.0, 1.0]`.
    pub wind_speed: f32,
    /// Mist / Depth macro in `[0.0, 1.0]`.
    pub mist_depth: f32,
    /// Abundance (trigger probability) macro in `[0.0, 1.0]`.
    pub abundance: f32,
    pub time: f32,
}

impl Garden {
    /// Construct a new Garden deterministically from a 6-character seed code.
    pub fn new(seed_code: String) -> Self {
        let rng = Xorshift64::from_garden_code(&seed_code);
        let mut garden = Self {
            seed_code,
            rng,
            season: Season::Mist,
            elements: Vec::with_capacity(64),
            ripples: Vec::with_capacity(64),
            wind_currents: [
                WindCurrent {
                    center_x: 18.0,
                    amplitude: 9.5,
                    wavelength: 55.0,
                    speed: 0.45,
                    phase: 0.0,
                },
                WindCurrent {
                    center_x: 52.0,
                    amplitude: 12.0,
                    wavelength: 70.0,
                    speed: 0.33,
                    phase: 2.1,
                },
                WindCurrent {
                    center_x: 84.0,
                    amplitude: 8.0,
                    wavelength: 45.0,
                    speed: 0.58,
                    phase: 4.2,
                },
            ],
            cursor_x: WORLD_WIDTH * 0.5,
            cursor_y: WORLD_HEIGHT * 0.5,
            selected_tool: ElementKind::Leaf,
            selected_macro: MacroControl::WindSpeed,
            wind_speed: 0.45,
            mist_depth: 0.68,
            abundance: 0.62,
            time: 0.0,
        };
        garden.populate_from_seed();
        garden
    }

    /// Generate an organic starter ecosystem from the current `seed_code`.
    pub fn populate_from_seed(&mut self) {
        self.elements.clear();
        self.ripples.clear();
        self.rng = Xorshift64::from_garden_code(&self.seed_code);

        // Pick starting season from seed
        self.season = match (self.rng.next_u64() % 4) as usize {
            0 => Season::Dawn,
            1 => Season::Mist,
            2 => Season::Dusk,
            _ => Season::Night,
        };

        // 2 grounding Stones
        for _ in 0..2 {
            let x = 18.0 + self.rng.next_f32() * 64.0;
            let y = 15.0 + self.rng.next_f32() * 45.0;
            self.elements.push(PlantedElement {
                kind: ElementKind::Stone,
                x,
                y,
                cooldown: 0.0,
                glow: 0.4,
                phase: self.rng.next_f32() * TAU,
            });
        }

        // 1 gentle Stream near one of the stones
        let stream_x = 25.0 + self.rng.next_f32() * 50.0;
        let stream_y = 20.0 + self.rng.next_f32() * 50.0;
        self.elements.push(PlantedElement {
            kind: ElementKind::Stream,
            x: stream_x,
            y: stream_y,
            cooldown: 0.0,
            glow: 0.5,
            phase: self.rng.next_f32() * TAU,
        });

        // 5 Leaves scattered across the wind currents
        for _ in 0..5 {
            let x = 12.0 + self.rng.next_f32() * 76.0;
            let y = 20.0 + self.rng.next_f32() * 65.0;
            self.elements.push(PlantedElement {
                kind: ElementKind::Leaf,
                x,
                y,
                cooldown: self.rng.next_f32() * 0.8,
                glow: 0.0,
                phase: self.rng.next_f32() * TAU,
            });
        }

        // 3 Stars high in the canopy
        for _ in 0..3 {
            let x = 15.0 + self.rng.next_f32() * 70.0;
            let y = 55.0 + self.rng.next_f32() * 36.0;
            self.elements.push(PlantedElement {
                kind: ElementKind::Star,
                x,
                y,
                cooldown: 0.5 + self.rng.next_f32() * 2.5,
                glow: 0.2,
                phase: self.rng.next_f32() * TAU,
            });
        }
    }

    /// Cycle to the next Season and re-quantize active Stones to the new harmonic mode.
    pub fn cycle_season(&mut self, bridge: &SharedAudioBridge) {
        self.season = self.season.next();
        self.sync_drones_and_streams(bridge);
    }

    /// Re-roll a new deterministic 6-character Garden Seed and re-populate.
    pub fn randomize_seed(&mut self, bridge: &SharedAudioBridge) {
        let next_raw = self.rng.next_u64() ^ 0xA5A5_5A5A_F0F0_0F0F;
        self.seed_code = Xorshift64::format_garden_code(next_raw);
        bridge.push_cmd(DspCommand::Reseed(next_raw));
        self.populate_from_seed();
        self.sync_drones_and_streams(bridge);
    }

    /// Plant an element of kind `kind` at `(x, y)` (or replace/limit Stones to `MAX_DRONES`).
    pub fn plant_at(
        &mut self,
        kind: ElementKind,
        x: f32,
        y: f32,
        bridge: &SharedAudioBridge,
    ) {
        let cx = x.clamp(3.0, WORLD_WIDTH - 3.0);
        let cy = y.clamp(4.0, WORLD_HEIGHT - 4.0);

        // If planting a Stone and already at MAX_DRONES (4), remove the oldest Stone so new one takes root
        if kind == ElementKind::Stone {
            let stone_count = self
                .elements
                .iter()
                .filter(|e| e.kind == ElementKind::Stone)
                .count();
            if stone_count >= MAX_DRONES {
                if let Some(pos) = self
                    .elements
                    .iter()
                    .position(|e| e.kind == ElementKind::Stone)
                {
                    self.elements.remove(pos);
                }
            }
        }

        let phase = self.rng.next_f32() * TAU;
        self.elements.push(PlantedElement {
            kind,
            x: cx,
            y: cy,
            cooldown: 0.4,
            glow: 1.0,
            phase,
        });

        // Give immediate gentle feedback ripple & sound on planting
        let y_norm = (cy / WORLD_HEIGHT).clamp(0.0, 1.0);
        let pan = ((cx / WORLD_WIDTH) * 2.0 - 1.0).clamp(-1.0, 1.0);

        match kind {
            ElementKind::Leaf => {
                let freq = quantize_pitch(self.season, y_norm, 1, 3);
                bridge.push_cmd(DspCommand::TriggerPluck {
                    freq,
                    pan,
                    velocity: 0.65,
                });
                self.spawn_ripple(cx, cy, 14.0, 0.55, ElementKind::Leaf);
            }
            ElementKind::Star => {
                let freq = quantize_pitch(self.season, y_norm, 2, 3);
                bridge.push_cmd(DspCommand::TriggerDroplet {
                    freq,
                    pan,
                    velocity: 0.6,
                });
                self.spawn_ripple(cx, cy, 18.0, 0.42, ElementKind::Star);
            }
            ElementKind::Stone | ElementKind::Stream => {
                self.spawn_ripple(cx, cy, 11.0, 0.65, kind);
            }
        }

        self.sync_drones_and_streams(bridge);
    }

    /// Remove the nearest element within `radius` of `(x, y)`. Returns `true` if removed.
    pub fn remove_nearest_at(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        bridge: &SharedAudioBridge,
    ) -> bool {
        let mut best_idx = None;
        let mut best_dist_sq = radius * radius;

        for (i, el) in self.elements.iter().enumerate() {
            let dx = el.x - x;
            let dy = el.y - y;
            let d2 = dx * dx + dy * dy;
            if d2 <= best_dist_sq {
                best_dist_sq = d2;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            self.elements.remove(idx);
            self.sync_drones_and_streams(bridge);
            true
        } else {
            false
        }
    }

    /// Clear all elements from the terrarium.
    pub fn clear_all(&mut self, bridge: &SharedAudioBridge) {
        self.elements.clear();
        self.ripples.clear();
        self.sync_drones_and_streams(bridge);
    }

    /// Spawn an expanding visual circle ripple.
    pub fn spawn_ripple(
        &mut self,
        x: f32,
        y: f32,
        max_radius: f32,
        decay_rate: f32,
        kind: ElementKind,
    ) {
        if self.ripples.len() >= 48 {
            self.ripples.remove(0);
        }
        self.ripples.push(VisualRipple {
            x,
            y,
            radius: 0.8,
            max_radius,
            life: 1.0,
            decay_rate,
            kind,
        });
    }

    /// Adjust the currently selected macro parameter by `delta`.
    pub fn adjust_selected_macro(&mut self, delta: f32, bridge: &SharedAudioBridge) {
        match self.selected_macro {
            MacroControl::WindSpeed => {
                self.wind_speed = (self.wind_speed + delta).clamp(0.05, 1.0);
            }
            MacroControl::MistDepth => {
                self.mist_depth = (self.mist_depth + delta).clamp(0.05, 1.0);
            }
            MacroControl::Abundance => {
                self.abundance = (self.abundance + delta).clamp(0.05, 1.0);
            }
        }
        bridge.push_cmd(DspCommand::SetMacros {
            wind_speed: self.wind_speed,
            mist_depth: self.mist_depth,
        });
    }

    /// Synchronize all active Stones (Drones) and Streams with the real-time DSP thread.
    pub fn sync_drones_and_streams(&self, bridge: &SharedAudioBridge) {
        // Collect all Streams first so we can compute proximity warmth on each Stone
        let mut streams: [(f32, f32); 32] = [(0.0, 0.0); 32];
        let mut stream_count = 0usize;
        for el in &self.elements {
            if el.kind == ElementKind::Stream && stream_count < streams.len() {
                streams[stream_count] = (el.x, el.y);
                stream_count += 1;
            }
        }

        // Stream pink-noise level scales smoothly with number of planted Streams
        let stream_level = if stream_count == 0 {
            0.0
        } else {
            (0.28 + (stream_count as f32 - 1.0) * 0.22).clamp(0.0, 1.0)
        };
        bridge.push_cmd(DspCommand::SetStreamLevel(stream_level));

        // Assign up to MAX_DRONES Stones to the 4 drone voice slots
        let mut slot = 0usize;
        for el in &self.elements {
            if el.kind == ElementKind::Stone && slot < MAX_DRONES {
                let y_norm = (el.y / WORLD_HEIGHT).clamp(0.0, 1.0);
                let freq = quantize_pitch(self.season, y_norm, 0, 2);
                let pan = ((el.x / WORLD_WIDTH) * 2.0 - 1.0).clamp(-1.0, 1.0);

                // Compute warmth boost from nearby Streams (within distance 35.0)
                let mut warmth = 0.0_f32;
                for &(sx, sy) in &streams[..stream_count] {
                    let dx = el.x - sx;
                    let dy = el.y - sy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist < 38.0 {
                        warmth += (1.0 - dist / 38.0).powi(2);
                    }
                }
                warmth = warmth.clamp(0.0, 1.0);

                bridge.push_cmd(DspCommand::SetDrone {
                    slot,
                    active: true,
                    freq,
                    pan,
                    stream_warmth: warmth,
                });
                slot += 1;
            }
        }

        // Silence remaining unused drone slots
        while slot < MAX_DRONES {
            bridge.push_cmd(DspCommand::SetDrone {
                slot,
                active: false,
                freq: 110.0,
                pan: 0.0,
                stream_warmth: 0.0,
            });
            slot += 1;
        }

        bridge.push_cmd(DspCommand::SetMacros {
            wind_speed: self.wind_speed,
            mist_depth: self.mist_depth,
        });
    }

    /// Step the 2D simulation forward by `dt` seconds.
    pub fn tick(&mut self, dt: f32, bridge: &SharedAudioBridge) {
        self.time += dt;

        // 1. Advance wind currents horizontally across the terrarium
        let wind_drift_scale = 3.5 + self.wind_speed * 14.5;
        for current in &mut self.wind_currents {
            current.center_x =
                (current.center_x + current.speed * wind_drift_scale * dt).rem_euclid(WORLD_WIDTH);
        }

        // 2. Check element interactions with wind waves and spontaneous Star shimmers
        let mut new_ripples: [(f32, f32, f32, f32, ElementKind); 16] =
            [(0.0, 0.0, 0.0, 0.0, ElementKind::Leaf); 16];
        let mut ripple_count = 0usize;

        let season = self.season;
        let abundance = self.abundance;
        let wind_speed = self.wind_speed;
        let time = self.time;
        let currents = self.wind_currents;

        for el in &mut self.elements {
            el.cooldown = (el.cooldown - dt).max(0.0);
            el.glow = (el.glow - dt * 0.75).max(0.0);

            match el.kind {
                ElementKind::Leaf => {
                    if el.cooldown <= 0.0 {
                        // Check if any wind current wave front intersects this Leaf
                        for current in &currents {
                            let wave_x = current.wave_x_at(el.y, time);
                            let dist_x = (wave_x - el.x).abs();
                            let wrapped_dist = dist_x.min(WORLD_WIDTH - dist_x);

                            if wrapped_dist < 1.6 {
                                el.cooldown = 1.6 - wind_speed * 0.7;
                                // Trigger probability governed by Abundance macro
                                if self.rng.next_f32() <= (0.18 + abundance * 0.82) {
                                    let y_norm = (el.y / WORLD_HEIGHT).clamp(0.0, 1.0);
                                    let freq = quantize_pitch(season, y_norm, 1, 3);
                                    let pan = ((el.x / WORLD_WIDTH) * 2.0 - 1.0).clamp(-1.0, 1.0);
                                    let velocity =
                                        0.38 + wind_speed * 0.35 + self.rng.next_f32() * 0.22;

                                    bridge.push_cmd(DspCommand::TriggerPluck {
                                        freq,
                                        pan,
                                        velocity,
                                    });
                                    el.glow = 1.0;

                                    if ripple_count < new_ripples.len() {
                                        new_ripples[ripple_count] = (
                                            el.x,
                                            el.y,
                                            11.0 + velocity * 10.0,
                                            0.48,
                                            ElementKind::Leaf,
                                        );
                                        ripple_count += 1;
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
                ElementKind::Star => {
                    if el.cooldown <= 0.0 {
                        // Spontaneous high-register crystalline droplet emission
                        let trigger_prob = dt * (0.14 + abundance * 0.55);
                        if self.rng.next_f32() < trigger_prob {
                            el.cooldown = 1.8 + self.rng.next_f32() * (3.5 - abundance * 2.0);
                            let y_norm = (el.y / WORLD_HEIGHT).clamp(0.0, 1.0);
                            let freq = quantize_pitch(season, y_norm, 2, 3);
                            let pan = ((el.x / WORLD_WIDTH) * 2.0 - 1.0).clamp(-1.0, 1.0);
                            let velocity = 0.35 + self.rng.next_f32() * 0.45;

                            bridge.push_cmd(DspCommand::TriggerDroplet {
                                freq,
                                pan,
                                velocity,
                            });
                            el.glow = 1.0;

                            if ripple_count < new_ripples.len() {
                                new_ripples[ripple_count] = (
                                    el.x,
                                    el.y,
                                    16.0 + velocity * 12.0,
                                    0.32,
                                    ElementKind::Star,
                                );
                                ripple_count += 1;
                            }
                        }
                    }
                }
                ElementKind::Stone | ElementKind::Stream => {
                    // Subtle breathing pulse when wind passes over a Stone or Stream
                    for current in &currents {
                        let wave_x = current.wave_x_at(el.y, time);
                        let dist_x = (wave_x - el.x).abs();
                        if dist_x < 1.5 {
                            el.glow = el.glow.max(0.55);
                        }
                    }
                }
            }
        }

        for &(rx, ry, max_r, decay, kind) in &new_ripples[..ripple_count] {
            self.spawn_ripple(rx, ry, max_r, decay, kind);
        }

        // 3. Update expanding ripples and prune expired ones
        for ripple in &mut self.ripples {
            ripple.life -= ripple.decay_rate * dt;
            let progress = (1.0 - ripple.life).clamp(0.0, 1.0);
            // Smooth ease-out expansion
            let ease = 1.0 - (1.0 - progress).powi(2);
            ripple.radius = 0.8 + ease * ripple.max_radius;
        }
        self.ripples.retain(|r| r.life > 0.02);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_garden_simulation_and_planting() {
        let bridge = SharedAudioBridge::new();
        let mut garden = Garden::new(String::from("KORES7"));
        garden.sync_drones_and_streams(&bridge);

        let initial_len = garden.elements.len();
        assert!(initial_len >= 8);

        garden.plant_at(ElementKind::Leaf, 50.0, 60.0, &bridge);
        assert_eq!(garden.elements.len(), initial_len + 1);
        assert!(!garden.ripples.is_empty());

        for _ in 0..120 {
            garden.tick(1.0 / 60.0, &bridge);
        }

        assert!(garden.remove_nearest_at(50.0, 60.0, 5.0, &bridge));
    }
}

