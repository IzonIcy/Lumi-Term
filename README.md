# Lumi-Term

A terminal emulator written in Rust. GPU-rendered, real PTY shell, custom VT parser.

## Why

I wanted to understand how terminals actually work — the whole stack from keystroke to character appearing on screen. Building one seemed like the best way to learn. It's not xterm-level complete but it runs a real shell, renders via the GPU, and handles most common escape sequences.

## How it works

```
PTY process ──→ Rust core (VT parsing, screen state) ──→ C FFI bridge ──→ GPU render
```

The PTY spawns whatever shell you have set, reads its output, parses escape sequences into screen state, and renders damage regions through a C bridge that talks to the GPU. The C code gets compiled at build time by Rust's build script.

## Run it

```bash
cargo run
```

Opens a terminal window connected to your default shell.

## Config

On first launch it creates a TOML config at:

- macOS: `~/Library/Application Support/com.lumi.lumi-term/lumi-term.toml`
- Linux: `~/.config/lumi/lumi-term/lumi-term.toml`

```toml
[window]
title = "Lumi-Term"
width = 1280.0
height = 760.0

[terminal]
font_size = 16.0
scrollback = 10000
shell = "/bin/zsh"

[theme]
background = [17, 17, 17]
foreground = [233, 233, 233]
```

## Code

```
src/
├── main.rs           # entry point
├── app.rs            # event loop, frame timing
├── pty.rs            # PTY spawn, I/O, signals
├── config.rs         # TOML loading
└── chrome_bridge.rs  # C FFI declarations

native/
├── lumi_chrome.c     # GPU rendering
└── lumi_chrome.h
```

## Build

```bash
cargo build --release
cargo clippy
```

## Requirements

Rust toolchain, C11 compiler (for the bridge), macOS 12+ or Linux with Wayland/X11.
