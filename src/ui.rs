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

//! Terminal UI & Seasonal TrueColor Renderer (`ui.rs`)
//!
//! Strictly a disposable view/controller layer over `garden.rs` and `dsp.rs`.
//! Renders the Braille `Canvas` terrarium, smooth RGB seasonal palettes,
//! expanding `Circle` ripples, interactive `Gauge` macros, breathing waveform,
//! and handles both keyboard and mouse click/drag events.

use std::f32::consts::TAU;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Points};
use ratatui::widgets::{Block, BorderType, Borders, Gauge, Paragraph};
use ratatui::Frame;

use crate::audio::WavRecorder;
use crate::dsp::{Season, SharedAudioBridge, WAVEFORM_BINS};
use crate::garden::{
    ElementKind, Garden, MacroControl, WORLD_HEIGHT, WORLD_WIDTH,
};

/// Smoothly interpolated RGB color in `[0.0, 255.0]`.
#[derive(Clone, Copy, Debug)]
pub struct RgbF32(pub f32, pub f32, pub f32);

impl RgbF32 {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(r as f32, g as f32, b as f32)
    }

    #[inline]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let k = t.clamp(0.0, 1.0);
        Self(
            self.0 + (other.0 - self.0) * k,
            self.1 + (other.1 - self.1) * k,
            self.2 + (other.2 - self.2) * k,
        )
    }

    #[inline]
    pub fn to_color(self) -> Color {
        Color::Rgb(
            self.0.clamp(0.0, 255.0) as u8,
            self.1.clamp(0.0, 255.0) as u8,
            self.2.clamp(0.0, 255.0) as u8,
        )
    }
}

/// Organic TrueColor palette for a Season.
#[derive(Clone, Copy, Debug)]
pub struct SeasonPalette {
    pub bg: RgbF32,
    pub border: RgbF32,
    pub text_primary: RgbF32,
    pub text_muted: RgbF32,
    pub wind: RgbF32,
    pub stone: RgbF32,
    pub leaf: RgbF32,
    pub stream: RgbF32,
    pub star: RgbF32,
    pub accent: RgbF32,
}

impl SeasonPalette {
    pub const fn for_season(season: Season) -> Self {
        match season {
            Season::Dawn => Self {
                bg: RgbF32::new(20, 18, 16),
                border: RgbF32::new(168, 136, 86),
                text_primary: RgbF32::new(244, 228, 196),
                text_muted: RgbF32::new(146, 130, 106),
                wind: RgbF32::new(92, 78, 56),
                stone: RgbF32::new(214, 166, 104),
                leaf: RgbF32::new(168, 212, 118),
                stream: RgbF32::new(118, 192, 188),
                star: RgbF32::new(255, 226, 142),
                accent: RgbF32::new(238, 188, 108),
            },
            Season::Mist => Self {
                bg: RgbF32::new(13, 21, 20),
                border: RgbF32::new(82, 146, 132),
                text_primary: RgbF32::new(208, 236, 226),
                text_muted: RgbF32::new(104, 142, 134),
                wind: RgbF32::new(48, 86, 80),
                stone: RgbF32::new(142, 176, 158),
                leaf: RgbF32::new(116, 218, 152),
                stream: RgbF32::new(92, 204, 218),
                star: RgbF32::new(204, 246, 238),
                accent: RgbF32::new(106, 198, 168),
            },
            Season::Dusk => Self {
                bg: RgbF32::new(22, 15, 26),
                border: RgbF32::new(156, 104, 142),
                text_primary: RgbF32::new(238, 212, 226),
                text_muted: RgbF32::new(138, 108, 134),
                wind: RgbF32::new(84, 54, 88),
                stone: RgbF32::new(218, 138, 98),
                leaf: RgbF32::new(234, 174, 108),
                stream: RgbF32::new(152, 132, 216),
                star: RgbF32::new(250, 204, 236),
                accent: RgbF32::new(212, 134, 166),
            },
            Season::Night => Self {
                bg: RgbF32::new(10, 13, 24),
                border: RgbF32::new(74, 96, 156),
                text_primary: RgbF32::new(204, 218, 248),
                text_muted: RgbF32::new(96, 112, 154),
                wind: RgbF32::new(40, 54, 96),
                stone: RgbF32::new(122, 144, 196),
                leaf: RgbF32::new(104, 198, 186),
                stream: RgbF32::new(88, 152, 232),
                star: RgbF32::new(236, 242, 255),
                accent: RgbF32::new(124, 162, 238),
            },
        }
    }

    pub fn lerp(self, target: Self, t: f32) -> Self {
        Self {
            bg: self.bg.lerp(target.bg, t),
            border: self.border.lerp(target.border, t),
            text_primary: self.text_primary.lerp(target.text_primary, t),
            text_muted: self.text_muted.lerp(target.text_muted, t),
            wind: self.wind.lerp(target.wind, t),
            stone: self.stone.lerp(target.stone, t),
            leaf: self.leaf.lerp(target.leaf, t),
            stream: self.stream.lerp(target.stream, t),
            star: self.star.lerp(target.star, t),
            accent: self.accent.lerp(target.accent, t),
        }
    }

    pub fn color_for_element(&self, kind: ElementKind) -> RgbF32 {
        match kind {
            ElementKind::Stone => self.stone,
            ElementKind::Leaf => self.leaf,
            ElementKind::Stream => self.stream,
            ElementKind::Star => self.star,
        }
    }
}

/// Cached layout rectangles for mouse hit-testing and smooth palette state.
pub struct UiState {
    pub palette: SeasonPalette,
    pub terrarium_rect: Rect,
    pub wind_gauge_rect: Rect,
    pub mist_gauge_rect: Rect,
    pub abundance_gauge_rect: Rect,
    pub waveform_buf: [f32; WAVEFORM_BINS],
    pub smoothed_waveform: [f32; WAVEFORM_BINS],
    pub recorder: Option<WavRecorder>,
    pub last_saved_notice: Option<(String, f32)>,
    pub should_quit: bool,
}

impl UiState {
    pub fn new(initial_season: Season) -> Self {
        Self {
            palette: SeasonPalette::for_season(initial_season),
            terrarium_rect: Rect::default(),
            wind_gauge_rect: Rect::default(),
            mist_gauge_rect: Rect::default(),
            abundance_gauge_rect: Rect::default(),
            waveform_buf: [0.0; WAVEFORM_BINS],
            smoothed_waveform: [0.0; WAVEFORM_BINS],
            recorder: None,
            last_saved_notice: None,
            should_quit: false,
        }
    }

    /// Smoothly transition the TrueColor palette toward the active Season, update waveform,
    /// and drain any recorded WAV frames to disk.
    pub fn update(&mut self, dt: f32, garden: &Garden, bridge: &SharedAudioBridge) {
        let target = SeasonPalette::for_season(garden.season);
        self.palette = self.palette.lerp(target, (dt * 3.2).clamp(0.0, 1.0));

        bridge.read_waveform(&mut self.waveform_buf);
        for i in 0..WAVEFORM_BINS {
            self.smoothed_waveform[i] =
                self.smoothed_waveform[i] * 0.75 + self.waveform_buf[i] * 0.25;
        }

        if let Some(ref mut rec) = self.recorder {
            let _ = rec.poll_and_write(bridge);
        }

        if let Some((_, ref mut ttl)) = self.last_saved_notice {
            *ttl -= dt;
            if *ttl <= 0.0 {
                self.last_saved_notice = None;
            }
        }
    }

    /// Toggle WAV session recording in `$HOME` on/off.
    pub fn toggle_recording(&mut self, garden: &Garden, bridge: &SharedAudioBridge) {
        if let Some(mut rec) = self.recorder.take() {
            if let Ok(saved_path) = rec.stop_and_finalize(bridge) {
                self.last_saved_notice = Some((saved_path, 6.0));
            }
        } else if let Ok(rec) = WavRecorder::start_in_home(&garden.seed_code, bridge) {
            self.last_saved_notice = None;
            self.recorder = Some(rec);
        }
    }

    /// Ensure any active WAV recording is cleanly finalized before exiting.
    pub fn finalize_recording_if_active(&mut self, bridge: &SharedAudioBridge) {
        if let Some(mut rec) = self.recorder.take() {
            let _ = rec.stop_and_finalize(bridge);
        }
    }

    /// Render the complete 3-section TUI layout.
    pub fn draw(&mut self, frame: &mut Frame, garden: &Garden) {
        let size = frame.area();

        // Fill terminal background with the active seasonal TrueColor background
        let bg_block = Block::default().style(Style::default().bg(self.palette.bg.to_color()));
        frame.render_widget(bg_block, size);

        // Divide terminal vertically into 3 sections:
        // 1. Main Terrarium View (Top 75%)
        // 2. Atmosphere & Macro Bar (Middle 15%)
        // 3. Footer (Bottom 10%, min 3 rows)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(75),
                Constraint::Percentage(15),
                Constraint::Percentage(10),
            ])
            .split(size);

        self.terrarium_rect = chunks[0];
        self.render_terrarium(frame, chunks[0], garden);
        self.render_atmosphere_bar(frame, chunks[1], garden);
        self.render_footer(frame, chunks[2], garden);
    }

    /// Section 1: Main Terrarium View (Top 75%) with `Marker::Braille` Canvas.
    fn render_terrarium(&self, frame: &mut Frame, area: Rect, garden: &Garden) {
        let pal = self.palette;
        let mut title_spans = vec![
            Span::styled(
                " ✿ BRYUM ",
                Style::default()
                    .fg(pal.accent.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "— Living Sound Terrarium ",
                Style::default().fg(pal.text_muted.to_color()),
            ),
            Span::styled(
                format!(" [Planting: {}] ", garden.selected_tool.label()),
                Style::default()
                    .fg(pal.color_for_element(garden.selected_tool).to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if let Some(ref rec) = self.recorder {
            let secs = rec.elapsed_secs();
            let mins = secs / 60;
            let rem_secs = secs % 60;
            let pulse = ((garden.time * 4.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
            let rec_color = RgbF32::new(235, 85, 85)
                .lerp(RgbF32::new(255, 165, 120), pulse)
                .to_color();
            title_spans.push(Span::styled(
                format!(" ● REC {:02}:{:02} ({}) ", mins, rem_secs, rec.display_name),
                Style::default().fg(rec_color).add_modifier(Modifier::BOLD),
            ));
        } else if let Some((ref path, _)) = self.last_saved_notice {
            title_spans.push(Span::styled(
                format!(" ✓ Saved {} ", path),
                Style::default()
                    .fg(pal.leaf.to_color())
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let title = Line::from(title_spans);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(pal.border.to_color()))
            .style(Style::default().bg(pal.bg.to_color()));

        let canvas = Canvas::default()
            .block(block)
            .marker(Marker::Braille)
            .background_color(pal.bg.to_color())
            .x_bounds([0.0, WORLD_WIDTH as f64])
            .y_bounds([0.0, WORLD_HEIGHT as f64])
            .paint(|ctx| {
                // 1. Render flowing Wind Currents as drifting sinusoidal waves of Braille dots
                let mut wind_pts_primary = [(0.0_f64, 0.0_f64); 240];
                let mut wind_pts_faint = [(0.0_f64, 0.0_f64); 240];
                let mut p_idx = 0usize;
                let mut f_idx = 0usize;

                for current in &garden.wind_currents {
                    for step in 0..80 {
                        let y = (step as f32 / 80.0) * WORLD_HEIGHT;
                        let x = current.wave_x_at(y, garden.time);
                        if p_idx < wind_pts_primary.len() {
                            wind_pts_primary[p_idx] = (x as f64, y as f64);
                            p_idx += 1;
                        }
                        // Secondary trailing mist filament
                        let trail_x = (x - 2.2).rem_euclid(WORLD_WIDTH);
                        if f_idx < wind_pts_faint.len() {
                            wind_pts_faint[f_idx] = (trail_x as f64, y as f64);
                            f_idx += 1;
                        }
                    }
                }

                ctx.draw(&Points {
                    coords: &wind_pts_faint[..f_idx],
                    color: pal.bg.lerp(pal.wind, 0.55).to_color(),
                });
                ctx.draw(&Points {
                    coords: &wind_pts_primary[..p_idx],
                    color: pal.wind.lerp(pal.accent, 0.25).to_color(),
                });

                // 2. Render expanding Visual Ripples (fading RGB toward background as envelope decays)
                for ripple in &garden.ripples {
                    let base_rgb = pal.color_for_element(ripple.kind);
                    let faded_rgb = pal.bg.lerp(base_rgb, ripple.life.clamp(0.0, 1.0));
                    ctx.draw(&Circle {
                        x: ripple.x as f64,
                        y: ripple.y as f64,
                        radius: ripple.radius as f64,
                        color: faded_rgb.to_color(),
                    });
                }

                // 3. Render planted nature elements with distinct Braille geometry & colors
                for el in &garden.elements {
                    let base_rgb = pal.color_for_element(el.kind);
                    let glow_rgb = base_rgb.lerp(pal.text_primary, el.glow * 0.65);
                    let color = glow_rgb.to_color();
                    let breathe = (garden.time * 1.8 + el.phase).sin();

                    match el.kind {
                        ElementKind::Stone => {
                            // Concentric grounded monolith rings
                            let r_outer = 2.6 + 0.35 * breathe;
                            ctx.draw(&Circle {
                                x: el.x as f64,
                                y: el.y as f64,
                                radius: r_outer as f64,
                                color,
                            });
                            ctx.draw(&Circle {
                                x: el.x as f64,
                                y: el.y as f64,
                                radius: 1.2,
                                color,
                            });
                        }
                        ElementKind::Leaf => {
                            // Swaying botanical leaf blade + stem
                            let sway = 1.1 * (garden.time * 2.4 + el.phase).sin();
                            ctx.draw(&CanvasLine {
                                x1: (el.x - 1.4) as f64,
                                y1: (el.y - 1.2) as f64,
                                x2: (el.x + 1.4 + sway) as f64,
                                y2: (el.y + 1.4) as f64,
                                color,
                            });
                            ctx.draw(&Circle {
                                x: (el.x + sway * 0.4) as f64,
                                y: el.y as f64,
                                radius: 1.35 + (el.glow * 0.6) as f64,
                                color,
                            });
                        }
                        ElementKind::Stream => {
                            // Flowing triple water ripples
                            for row in -1..=1 {
                                let y_off = row as f32 * 1.1;
                                let wave = 0.8 * (garden.time * 3.0 + el.phase + row as f32).sin();
                                ctx.draw(&CanvasLine {
                                    x1: (el.x - 2.6 + wave) as f64,
                                    y1: (el.y + y_off) as f64,
                                    x2: (el.x + 2.6 + wave) as f64,
                                    y2: (el.y + y_off) as f64,
                                    color,
                                });
                            }
                        }
                        ElementKind::Star => {
                            // Twinkling 4-point celestial spore cross
                            let arm = 1.8 + 0.5 * breathe + el.glow * 1.1;
                            ctx.draw(&CanvasLine {
                                x1: (el.x - arm) as f64,
                                y1: el.y as f64,
                                x2: (el.x + arm) as f64,
                                y2: el.y as f64,
                                color,
                            });
                            ctx.draw(&CanvasLine {
                                x1: el.x as f64,
                                y1: (el.y - arm) as f64,
                                x2: el.x as f64,
                                y2: (el.y + arm) as f64,
                                color,
                            });
                        }
                    }
                }

                // 4. Render gentle planting cursor reticle
                let cursor_color = pal.color_for_element(garden.selected_tool).to_color();
                let cx = garden.cursor_x as f64;
                let cy = garden.cursor_y as f64;
                ctx.draw(&Circle {
                    x: cx,
                    y: cy,
                    radius: 1.9,
                    color: cursor_color,
                });
            });

        frame.render_widget(canvas, area);
    }

    /// Section 2: Atmosphere & Macro Bar (Middle 15%), divided horizontally into
    /// Left (3 macro Gauges) and Right (smooth Braille breathing waveform).
    fn render_atmosphere_bar(&mut self, frame: &mut Frame, area: Rect, garden: &Garden) {
        let pal = self.palette;
        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(area);

        // Left: 3 simple macro controls rendered with Gauge
        let macro_block = Block::default()
            .title(Span::styled(
                " Atmosphere ",
                Style::default()
                    .fg(pal.text_primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(pal.border.to_color()))
            .style(Style::default().bg(pal.bg.to_color()));

        let inner_macros = macro_block.inner(horiz[0]);
        frame.render_widget(macro_block, horiz[0]);

        let gauge_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .split(inner_macros);

        self.wind_gauge_rect = gauge_cols[0];
        self.mist_gauge_rect = gauge_cols[1];
        self.abundance_gauge_rect = gauge_cols[2];

        self.render_single_gauge(
            frame,
            gauge_cols[0],
            "Wind Speed",
            garden.wind_speed,
            garden.selected_macro == MacroControl::WindSpeed,
            pal.stream,
        );
        self.render_single_gauge(
            frame,
            gauge_cols[1],
            "Mist / Depth",
            garden.mist_depth,
            garden.selected_macro == MacroControl::MistDepth,
            pal.leaf,
        );
        self.render_single_gauge(
            frame,
            gauge_cols[2],
            "Abundance",
            garden.abundance,
            garden.selected_macro == MacroControl::Abundance,
            pal.star,
        );

        // Right: Smooth Braille breathing waveform of the audio output
        let wave_canvas = Canvas::default()
            .block(
                Block::default()
                    .title(Span::styled(
                        " Garden Breath ",
                        Style::default()
                            .fg(pal.text_primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(pal.border.to_color()))
                    .style(Style::default().bg(pal.bg.to_color())),
            )
            .marker(Marker::Braille)
            .background_color(pal.bg.to_color())
            .x_bounds([0.0, (WAVEFORM_BINS - 1) as f64])
            .y_bounds([-1.0, 1.0])
            .paint(|ctx| {
                let breath_color = pal.accent.to_color();
                let mirror_color = pal.bg.lerp(pal.stream, 0.55).to_color();

                for i in 0..(WAVEFORM_BINS - 1) {
                    let t0 = i as f32 / WAVEFORM_BINS as f32;
                    let t1 = (i + 1) as f32 / WAVEFORM_BINS as f32;
                    let slow_tide0 = 0.18 * (garden.time * 1.4 + t0 * TAU).sin();
                    let slow_tide1 = 0.18 * (garden.time * 1.4 + t1 * TAU).sin();

                    let y0 = (self.smoothed_waveform[i] * 3.2 + slow_tide0).clamp(-0.92, 0.92);
                    let y1 = (self.smoothed_waveform[i + 1] * 3.2 + slow_tide1).clamp(-0.92, 0.92);

                    ctx.draw(&CanvasLine {
                        x1: i as f64,
                        y1: -y0 as f64 * 0.6,
                        x2: (i + 1) as f64,
                        y2: -y1 as f64 * 0.6,
                        color: mirror_color,
                    });
                    ctx.draw(&CanvasLine {
                        x1: i as f64,
                        y1: y0 as f64,
                        x2: (i + 1) as f64,
                        y2: y1 as f64,
                        color: breath_color,
                    });
                }
            });

        frame.render_widget(wave_canvas, horiz[1]);
    }

    fn render_single_gauge(
        &self,
        frame: &mut Frame,
        area: Rect,
        name: &str,
        value: f32,
        selected: bool,
        fill_rgb: RgbF32,
    ) {
        let pal = self.palette;
        let pct = (value.clamp(0.0, 1.0) * 100.0).round() as u16;
        let title_style = if selected {
            Style::default()
                .fg(pal.accent.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(pal.text_muted.to_color())
        };
        let prefix = if selected { "▸ " } else { "  " };

        let gauge = Gauge::default()
            .block(
                Block::default()
                    .title(Span::styled(format!("{prefix}{name} "), title_style))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(if selected {
                        pal.accent.to_color()
                    } else {
                        pal.bg.lerp(pal.border, 0.45).to_color()
                    })),
            )
            .gauge_style(
                Style::default()
                    .fg(fill_rgb.to_color())
                    .bg(pal.bg.lerp(fill_rgb, 0.18).to_color()),
            )
            .percent(pct)
            .label(Span::styled(
                format!("{pct}%"),
                Style::default()
                    .fg(pal.text_primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ));

        frame.render_widget(gauge, area);
    }

    /// Section 3: Footer (Bottom 10%) — Single-line legend with Keyboard + Mouse controls,
    /// active Season, and the 6-character deterministic Garden Seed.
    fn render_footer(&self, frame: &mut Frame, area: Rect, garden: &Garden) {
        let pal = self.palette;
        let legend = Line::from(vec![
            Span::styled(
                " [1]Stone [2]Leaf [3]Stream [4]Star ",
                Style::default().fg(pal.text_primary.to_color()),
            ),
            Span::styled("│ ", Style::default().fg(pal.border.to_color())),
            Span::styled(
                "Click/Enter: Plant  Right-Click/Del: Uproot  Tab/[-+]: Atmosphere ",
                Style::default().fg(pal.text_muted.to_color()),
            ),
            Span::styled("│ ", Style::default().fg(pal.border.to_color())),
            Span::styled("Season(Space): ", Style::default().fg(pal.text_muted.to_color())),
            Span::styled(
                format!("{} ", garden.season.name()),
                Style::default()
                    .fg(pal.accent.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│ ", Style::default().fg(pal.border.to_color())),
            Span::styled("Seed(R): ", Style::default().fg(pal.text_muted.to_color())),
            Span::styled(
                format!("{} ", garden.seed_code),
                Style::default()
                    .fg(pal.star.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("│ ", Style::default().fg(pal.border.to_color())),
            if self.recorder.is_some() {
                Span::styled(
                    "[O]Stop WAV ",
                    Style::default()
                        .fg(Color::Rgb(245, 110, 110))
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("[O]Record WAV ", Style::default().fg(pal.text_primary.to_color()))
            },
            Span::styled("│ ", Style::default().fg(pal.border.to_color())),
            Span::styled("[Q]Quit", Style::default().fg(pal.text_muted.to_color())),
        ]);

        let footer = Paragraph::new(legend)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(pal.border.to_color()))
                    .style(Style::default().bg(pal.bg.to_color())),
            );

        frame.render_widget(footer, area);
    }

    /// Handle keyboard input events.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        garden: &mut Garden,
        bridge: &SharedAudioBridge,
    ) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.finalize_recording_if_active(bridge);
            self.should_quit = true;
            return;
        }

        let step = 3.2;
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                self.finalize_recording_if_active(bridge);
                self.should_quit = true;
            }
            // Select Element Type (1..4)
            KeyCode::Char('1') => garden.selected_tool = ElementKind::Stone,
            KeyCode::Char('2') => garden.selected_tool = ElementKind::Leaf,
            KeyCode::Char('3') => garden.selected_tool = ElementKind::Stream,
            KeyCode::Char('4') => garden.selected_tool = ElementKind::Star,

            // Move cursor
            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('h') => {
                garden.cursor_x = (garden.cursor_x - step).clamp(3.0, WORLD_WIDTH - 3.0);
            }
            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('l') => {
                garden.cursor_x = (garden.cursor_x + step).clamp(3.0, WORLD_WIDTH - 3.0);
            }
            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('k') => {
                garden.cursor_y = (garden.cursor_y + step).clamp(4.0, WORLD_HEIGHT - 4.0);
            }
            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('j') => {
                garden.cursor_y = (garden.cursor_y - step).clamp(4.0, WORLD_HEIGHT - 4.0);
            }

            // Plant / Remove at cursor
            KeyCode::Enter | KeyCode::Char('p') | KeyCode::Char('P') => {
                garden.plant_at(
                    garden.selected_tool,
                    garden.cursor_x,
                    garden.cursor_y,
                    bridge,
                );
            }
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('x') | KeyCode::Char('X') => {
                garden.remove_nearest_at(garden.cursor_x, garden.cursor_y, 8.0, bridge);
            }

            // Cycle Season
            KeyCode::Char(' ') => {
                garden.cycle_season(bridge);
            }

            // Macro navigation & adjustment
            KeyCode::Tab => {
                garden.selected_macro = garden.selected_macro.next();
            }
            KeyCode::BackTab => {
                garden.selected_macro = garden.selected_macro.prev();
            }
            KeyCode::Char('[') | KeyCode::Char('-') | KeyCode::Char('_') => {
                garden.adjust_selected_macro(-0.05, bridge);
            }
            KeyCode::Char(']') | KeyCode::Char('=') | KeyCode::Char('+') => {
                garden.adjust_selected_macro(0.05, bridge);
            }

            // Re-seed, Record WAV to $HOME, or Clear
            KeyCode::Char('r') | KeyCode::Char('R') => {
                garden.randomize_seed(bridge);
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                self.toggle_recording(garden, bridge);
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                garden.clear_all(bridge);
            }
            _ => {}
        }
    }

    /// Handle mouse click, drag, and scroll events inside the Terrarium Canvas and Macro Gauges.
    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        garden: &mut Garden,
        bridge: &SharedAudioBridge,
    ) {
        let col = mouse.column;
        let row = mouse.row;

        // 1. Check if clicking or dragging inside the Main Terrarium Canvas
        let inner_terrarium = Rect {
            x: self.terrarium_rect.x.saturating_add(1),
            y: self.terrarium_rect.y.saturating_add(1),
            width: self.terrarium_rect.width.saturating_sub(2),
            height: self.terrarium_rect.height.saturating_sub(2),
        };

        if inner_terrarium.width > 0
            && inner_terrarium.height > 0
            && col >= inner_terrarium.x
            && col < inner_terrarium.x + inner_terrarium.width
            && row >= inner_terrarium.y
            && row < inner_terrarium.y + inner_terrarium.height
        {
            let norm_x =
                (col - inner_terrarium.x) as f32 / (inner_terrarium.width.max(1) as f32);
            // Invert terminal row so higher Y on screen = higher Y in world
            let norm_y = 1.0
                - ((row - inner_terrarium.y) as f32 / (inner_terrarium.height.max(1) as f32));

            let wx = (norm_x * WORLD_WIDTH).clamp(3.0, WORLD_WIDTH - 3.0);
            let wy = (norm_y * WORLD_HEIGHT).clamp(4.0, WORLD_HEIGHT - 4.0);

            garden.cursor_x = wx;
            garden.cursor_y = wy;

            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    garden.plant_at(garden.selected_tool, wx, wy, bridge);
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    // Avoid stacking duplicate elements on top of the exact same spot during drag
                    let too_close = garden.elements.iter().any(|el| {
                        let dx = el.x - wx;
                        let dy = el.y - wy;
                        dx * dx + dy * dy < 36.0
                    });
                    if !too_close {
                        garden.plant_at(garden.selected_tool, wx, wy, bridge);
                    }
                }
                MouseEventKind::Down(MouseButton::Right)
                | MouseEventKind::Drag(MouseButton::Right) => {
                    garden.remove_nearest_at(wx, wy, 9.0, bridge);
                }
                MouseEventKind::ScrollUp => {
                    garden.adjust_selected_macro(0.04, bridge);
                }
                MouseEventKind::ScrollDown => {
                    garden.adjust_selected_macro(-0.04, bridge);
                }
                _ => {}
            }
            return;
        }

        // 2. Check if clicking or dragging inside one of the 3 Macro Gauges
        if matches!(
            mouse.kind,
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
        ) {
            if let Some(ratio) = rect_horizontal_ratio(self.wind_gauge_rect, col, row) {
                garden.selected_macro = MacroControl::WindSpeed;
                garden.wind_speed = ratio.clamp(0.05, 1.0);
                garden.sync_drones_and_streams(bridge);
            } else if let Some(ratio) = rect_horizontal_ratio(self.mist_gauge_rect, col, row) {
                garden.selected_macro = MacroControl::MistDepth;
                garden.mist_depth = ratio.clamp(0.05, 1.0);
                garden.sync_drones_and_streams(bridge);
            } else if let Some(ratio) = rect_horizontal_ratio(self.abundance_gauge_rect, col, row) {
                garden.selected_macro = MacroControl::Abundance;
                garden.abundance = ratio.clamp(0.05, 1.0);
            }
        }
    }
}

fn rect_horizontal_ratio(rect: Rect, col: u16, row: u16) -> Option<f32> {
    if rect.width > 2
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
    {
        let inner_w = (rect.width - 2).max(1) as f32;
        let rel = (col.saturating_sub(rect.x + 1)) as f32 / inner_w;
        Some(rel.clamp(0.0, 1.0))
    } else {
        None
    }
}
