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

//! BRYUM — Generative Sound Garden in Rust (`ratatui` + Pure DSP)
//!
//! Entry point (`main.rs`) wiring the pure mathematical simulation (`garden.rs`),
//! zero-allocation procedural synthesizer (`dsp.rs`), audio backend (`audio.rs`),
//! and TrueColor Braille TUI (`ui.rs`).

pub mod audio;
pub mod dsp;
pub mod garden;
pub mod ui;

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::dsp::{SharedAudioBridge, Xorshift64};
use crate::garden::Garden;
use crate::ui::UiState;

fn main() -> io::Result<()> {
    let mut stdout_pcm = false;
    let mut seed_code = String::from("KORES7");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdout-pcm" => {
                stdout_pcm = true;
            }
            "--seed" => {
                if let Some(code) = args.next() {
                    let cleaned: String = code
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .map(|c| c.to_ascii_uppercase())
                        .take(6)
                        .collect();
                    if !cleaned.is_empty() {
                        seed_code = format!("{cleaned:K<6}");
                    }
                }
            }
            "--version" | "-V" | "--license" | "--warranty" => {
                println!("BRYUM {} — Generative Ambient Sound Garden", env!("CARGO_PKG_VERSION"));
                println!("Copyright (C) 2026 Voidwave Lab");
                println!("License GPLv3+: GNU GPL version 3 or later <https://gnu.org/licenses/gpl.html>");
                println!("This is free software: you are free to change and redistribute it.");
                println!("There is NO WARRANTY, to the extent permitted by law.");
                return Ok(());
            }
            "--help" | "-h" => {
                println!("BRYUM — Generative Ambient Sound Garden");
                println!("Copyright (C) 2026 Voidwave Lab (Licensed under GNU GPLv3+)");
                println!();
                println!("USAGE:");
                println!("  bryum [OPTIONS]");
                println!();
                println!("OPTIONS:");
                println!("  --seed <CODE>   6-character deterministic Garden Seed (default: KORES7)");
                println!("  --stdout-pcm    Bypass cpal & TUI; stream raw stereo f32le PCM (48kHz) to stdout");
                println!("                  Example: ./bryum --stdout-pcm | ffplay -f f32le -ar 48000 -ch_layout stereo -");
                println!("  -V, --version   Show version, copyright, and GNU GPLv3 warranty notice");
                println!("  -h, --help      Show this help message");
                return Ok(());
            }
            _ => {}
        }
    }

    let mut seed_rng = Xorshift64::from_garden_code(&seed_code);
    let numeric_seed = seed_rng.next_u64();

    // Fallback Unix pipe mode: stream raw f32le stereo PCM directly to stdout
    if stdout_pcm {
        return audio::run_stdout_pcm_stream(seed_code, numeric_seed);
    }

    // Standard interactive TUI + native cpal audio mode
    let bridge = Arc::new(SharedAudioBridge::new());
    let mut garden = Garden::new(seed_code);
    garden.sync_drones_and_streams(&bridge);

    let _audio_handle = audio::start_cpal_audio(Arc::clone(&bridge), numeric_seed);

    // Install panic hook so terminal raw mode & mouse capture are always restored cleanly
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut ui_state = UiState::new(garden.season);
    let tick_rate = Duration::from_millis(16); // ~60 FPS
    let mut last_frame = Instant::now();

    let run_result = (|| -> io::Result<()> {
        while !ui_state.should_quit {
            let now = Instant::now();
            let dt = now.duration_since(last_frame).as_secs_f32().clamp(0.001, 0.1);
            last_frame = now;

            // Advance garden physics, wind waves, ripples & DSP triggers
            garden.tick(dt, &bridge);
            ui_state.update(dt, &garden, &bridge);

            // Render TUI frame
            terminal.draw(|frame| {
                ui_state.draw(frame, &garden);
            })?;

            // Poll input events within remaining frame budget
            let timeout = tick_rate.saturating_sub(last_frame.elapsed());
            if event::poll(timeout)? {
                loop {
                    match event::read()? {
                        Event::Key(key) => {
                            // Only handle key press events (ignore release/repeat where applicable)
                            if key.kind == event::KeyEventKind::Press {
                                ui_state.handle_key(key, &mut garden, &bridge);
                            }
                        }
                        Event::Mouse(mouse) => {
                            ui_state.handle_mouse(mouse, &mut garden, &bridge);
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                    if ui_state.should_quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();

    // Restore terminal state
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();

    run_result
}
