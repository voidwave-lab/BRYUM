# ✿ BRYUM — Generative Sound Garden in the Terminal

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
![Rust Edition](https://img.shields.io/badge/Rust-2021-orange.svg) [![CI & Release Binaries](https://github.com/voidwave-lab/BRYUM/actions/workflows/rust.yml/badge.svg)](https://github.com/voidwave-lab/BRYUM/actions/workflows/rust.yml)

**BRYUM** (named after the resilient genus of mosses) is a hypnotic, terminal-based (TUI) generative ambient sound garden written in Rust. Designed from the ground up for non-musicians, it replaces technical synthesizer panels with a living visual terrarium where you plant **Stones**, **Leaves**, **Streams**, and **Stars** brushed by slow-drifting wind currents.

---

## 1. Core Philosophy

### Nature Metaphors, Zero Jargon
Traditional synthesizers and generative music tools confront users with engineering terminology—*oscillators, Hz, cutoff frequencies, Q factors, LFOs, FM ratios, and scale degrees*. **BRYUM** strips away all technical vocabulary. You never turn a frequency knob or program a sequencer; instead, you cultivate a 2D ecosystem:
- Planting higher in the canopy naturally produces brighter, higher-pitched tones.
- Planting to the left or right places the sound in the stereo field.
- Placing a **Stream** near a **Stone** organically warms and opens the stone's resonance.
- **Wind Currents** drift across the terrarium as sinusoidal Braille waves, brushing against **Leaves** to awaken plucked wooden/string chimes and spawning expanding visual ripples that fade into the background at the exact rate the sound decays.

### Mathematical Consonance
It is mathematically impossible to play a "wrong" or dissonant note in **BRYUM**. Every vertical coordinate in the terrarium is continuously quantized against the active **Season** (*Dawn*, *Mist*, *Dusk*, or *Night*). Changing the season smoothly shifts both the TrueColor visual palette of the terminal and the harmonic gravity of every element in the garden.

### Decoupled Longevity Architecture
Terminal UI libraries and operating system audio APIs evolve and churn over time, whereas pure mathematics does not. **BRYUM** enforces a strict architectural boundary:
1. **Zero-Dependency Core (`src/dsp.rs` & `src/garden.rs`):** Both the 2D ecological simulation and the entire real-time procedural synthesizer are written in 100% pure Rust using **only `std`/`core`**—zero external crates.
2. **Zero Audio-Thread Heap Allocations:** All oscillator banks, Karplus-Strong delay lines, state-variable filters, 4×4 feedback delay networks, and the Single-Producer Single-Consumer (SPSC) command ringbuffer are pre-allocated in fixed-size inline arrays. The real-time audio callback never allocates, locks a mutex, or blocks.
3. **Disposable View & Unix Pipe Longevity (`src/ui.rs` & `--stdout-pcm`):** `ratatui` and `crossterm` act strictly as a view/controller layer over the mathematical core. Furthermore, the `--stdout-pcm` flag bypasses both the TUI and native audio drivers to stream raw 32-bit floating-point stereo PCM (`f32le` at 48,000 Hz) directly to `stdout`, ensuring the sound garden can be piped into standard Unix audio tools (`ffplay`, `aplay`, `sox`, `ffmpeg`) decades into the future.

---

## 2. What BRYUM Implements

### The Four Terrarium Elements (`src/garden.rs`)

| Key | Element | Acoustic Role | Ecological Behavior & Synthesis |
| :-: | :--- | :--- | :--- |
| **`1`** | **Stone** *(Root / Drone)* | Warm, grounding low-register foundation | Sustains an evolving low-register drone. Vertical position selects a root interval; horizontal position pans across stereo. Up to 4 Stones resonate simultaneously. |
| **`2`** | **Leaf** *(Chime / Resonator)* | Plucked modal / string resonance | Silent while still. Whenever a drifting **Wind Current** intersects a Leaf, it triggers an extended Karplus-Strong plucked tone and spawns an expanding Braille circle ripple. |
| **`3`** | **Stream** *(Texture / Flow)* | Organic water & wind bed | Generates filtered pink noise that breathes across the stereo field and gently opens the low-pass filter warmth of any nearby **Stones** within its proximity radius. |
| **`4`** | **Star** *(Spore / Shimmer)* | High-register crystalline droplets | Hovering in the upper canopy, Stars spontaneously emit delicate 2-operator FM bell droplets with long exponential decays at organic intervals. |

### The Four Seasons (`SPACE` Key)

Pressing **`Space`** cycles through four atmospheric themes, smoothly interpolating both the terminal's TrueColor (`Color::Rgb`) palette and the underlying musical mode:

1. **Dawn** — *Lydian Mode* (`1, 2, 3, #4, 5, 6, 7` rooted on D2): Warm sunlit gold, amber bark, and bright moss green.
2. **Mist** — *Dorian Mode* (`1, 2, ♭3, 4, 5, 6, ♭7` rooted on C2): Deep forest moss, cyan mist, and cool slate.
3. **Dusk** — *Minor Pentatonic* (`1, ♭3, 4, 5, ♭7` rooted on E♭2): Twilight violet, warm terracotta, and autumn amber.
4. **Night** — *Hirajoshi Pentatonic* (`1, 2, ♭3, 5, ♭6` rooted on B1): Deep midnight indigo, lunar silver, and starlight cyan.

### Pure Procedural DSP Engine (`src/dsp.rs`)

All synthesis and effects processing is implemented from scratch in `src/dsp.rs`:
- **Deterministic `Xorshift64` PRNG:** Seeded by a human-readable 6-character alphanumeric **Garden Seed** (such as `KORES7`). Sharing a seed reproduces the exact initial ecosystem and random sequence.
- **4× Drone Voices:** Dual detuned saw/triangle hybrid oscillators paired with a sub-octave sine wave, shaped by a 2-pole **Chamberlain State-Variable Filter (SVF)** whose cutoff is modulated by a slow internal breath oscillator, the global **Mist / Depth** macro, and the proximity of nearby **Streams**.
- **12× Polyphonic Resonator Pool** (with oldest-voice stealing):
  - *Extended Karplus-Strong Plucked String:* Excited by a windowed low-passed noise burst + fundamental sine pulse, fed through a one-zero/one-pole loop damping filter, an allpass dispersion filter for woody/metallic warmth, and a resonant body SVF.
  - *2-Operator Crystalline FM:* Consonant harmonic modulator ratios (`1.5`, `2.0`, `3.0`, `4.0`) with independent modulator and carrier exponential decay envelopes.
- **Paul Kellet's 7-State Pink Noise Stream:** White noise shaped through Paul Kellet's refined pink-noise filter (`±0.05 dB` accuracy) and routed into dual stereo wandering bandpass Chamberlain SVFs.
- **Global Stereo Reverb (4×4 Hadamard FDN):** Stereo input diffusion via cascaded Schroeder allpass filters feeding a 4×4 orthogonal Hadamard Feedback Delay Network with mutually prime delay lines, linear-interpolated LFO modulation, high-frequency damping, and **6 to 15 seconds** of lush ambient decay, finished with a smooth Padé rational soft-clipper.

### Three-Tier `ratatui` Braille Interface (`src/ui.rs`)

1. **Main Terrarium View (Top 75%):** Sub-character high-resolution `Marker::Braille` canvas rendering drifting wind filaments, swaying botanical geometry for Stones/Leaves/Streams/Stars, and expanding `Canvas::Circle` ripples whose RGB values lerp toward the seasonal background as their audio envelopes decay.
2. **Atmosphere & Macro Bar (Middle 15%):**
   - *Left:* Three interactive `Gauge` controls (**Wind Speed**, **Mist / Depth**, and **Abundance**) adjustable via keyboard or direct mouse click/drag.
   - *Right:* **Garden Breath**, a dual-trace Braille waveform scope visualizing the real-time breathing of the audio output.
3. **Footer Bar (Bottom 10%):** Single-line control legend, active **Season** indicator, and the 6-character **Garden Seed**.

---

## 3. Building & Running

### Prerequisites
- Rust toolchain (`rustc` / `cargo` 1.75+, 2021 edition)
- A terminal emulator with TrueColor (24-bit RGB) and Unicode Braille font support
- On Linux, ALSA development headers (`libasound2-dev` or `alsa-lib-devel`) for `cpal`

### Build & Run (Interactive TUI + Native Audio)
```sh
cargo run --release
```

Start with a specific 6-character deterministic **Garden Seed**:
```sh
cargo run --release -- --seed KORES7
```

### Headless Unix Pipe Mode (`--stdout-pcm`)
Bypass `cpal` and the TUI to stream raw stereo 32-bit floating-point PCM (`f32le`, 48,000 Hz, 2 channels) directly to `stdout`:
```sh
# Pipe live into ffplay:
cargo run --release -- --stdout-pcm | ffplay -f f32le -ar 48000 -ch_layout stereo -

# Record 30 seconds of a deterministic garden seed to a WAV file via ffmpeg:
cargo run --release -- --seed KORES7 --stdout-pcm | ffmpeg -f f32le -ar 48000 -ac 2 -t 30 -i - garden_kores7.wav
```

---

## 4. Controls (Keyboard & Mouse)

| Input | Action |
| :--- | :--- |
| **`1` / `2` / `3` / `4`** | Select element to plant: **Stone** (`1`), **Leaf** (`2`), **Stream** (`3`), **Star** (`4`) |
| **Mouse Left Click / Drag** | Plant the selected element at the clicked position (or drag to adjust an **Atmosphere** gauge) |
| **Mouse Right Click / Drag** | Uproot (remove) the nearest element in the terrarium |
| **Mouse Scroll Wheel** | Adjust the currently selected **Atmosphere** macro |
| **Arrow Keys / `WASD` / `HJKL`** | Move the planting reticle inside the terrarium |
| **`Enter` / `P`** | Plant the selected element at the cursor reticle |
| **`Backspace` / `Delete` / `X`** | Uproot the nearest element to the cursor reticle |
| **`Space`** | Cycle **Season** (*Dawn* → *Mist* → *Dusk* → *Night*) |
| **`Tab` / `Shift+Tab`** | Cycle active Atmosphere macro (**Wind Speed** → **Mist / Depth** → **Abundance**) |
| **`-` / `+`** (or **`[` / `]`**) | Decrease / increase the selected Atmosphere macro |
| **`R`** | Generate a new deterministic 6-character **Garden Seed** and repopulate |
| **`C`** | Clear all elements from the terrarium |
| **`Q` / `Esc`** | Quit cleanly |

---

## 5. Project Structure & Supported Targets

```text
BRYUM/
├── .github/workflows/rust.yml # 16-target CI & Release binary matrix (macOS, Linux, BSD, Windows, ARM/Pi)
├── Cargo.toml                 # Strictly 3 dependencies: ratatui, crossterm, cpal (GPL-3.0-or-later)
├── Cross.toml                 # Multiarch ALSA cross-compilation config for cross-rs
├── LICENSE                    # Full GNU General Public License v3.0 text
├── README.md                  # Project documentation and philosophy
└── src/
    ├── main.rs                # CLI parser (--seed, --stdout-pcm, --version), terminal lifecycle & 60 FPS loop
    ├── dsp.rs                 # Zero-dependency, zero-allocation procedural DSP engine & SPSC atomic bridge
    ├── garden.rs              # Zero-dependency 2D ecological simulation, wind waves, ripples & pitch quantizer
    ├── ui.rs                  # ratatui Braille Canvas renderer, TrueColor seasonal palettes & mouse/key controller
    └── audio.rs               # Native cpal stream + OpenBSD/musl native audio pipe fallback + --stdout-pcm
```

### Pre-Built Release Matrix (`.github/workflows/rust.yml`)

| Platform Family | Target Triple(s) | Notes |
| :--- | :--- | :--- |
| **macOS** | `x86_64-apple-darwin`, `aarch64-apple-darwin` | Intel & Apple Silicon (M1–M4), CoreAudio via `cpal` |
| **Windows** | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | x64 & Windows on ARM64, WASAPI via `cpal` |
| **Linux (`glibc`)** | `x86_64-unknown-linux-gnu`, `i686-unknown-linux-gnu` | Ubuntu, Debian, Fedora, Arch, RHEL, openSUSE (ALSA via `cpal`) |
| **Linux (`musl` Static)** | `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` | Zero-dependency static ELF binaries for *any* Linux distro (Alpine, Void, NixOS, etc.) |
| **Raspberry Pi & SBCs** | `aarch64-unknown-linux-gnu`, `armv7-unknown-linux-gnueabihf`, `armv7-unknown-linux-musleabihf`, `arm-unknown-linux-gnueabihf`, `arm-unknown-linux-musleabihf`, `riscv64gc-unknown-linux-gnu` | Covers Raspberry Pi 1/Zero (ARMv6), Pi 2/3/4 32-bit (ARMv7), Pi 3/4/5 64-bit (ARM64), and 64-bit RISC-V boards |
| **FreeBSD & OpenBSD** | `x86_64-unknown-freebsd`, `x86_64-unknown-openbsd` | Native & cross-compiled BSD releases with `sndio` (`aucat`) / PulseAudio / ALSA pipe support |

---

## 6. License

**BRYUM** is free software: you can redistribute it and/or modify it under the terms of the **GNU General Public License** as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

See [LICENSE](./LICENSE) for the full license text.
