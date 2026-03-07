# Verbinal for Linux

A native Linux desktop companion for the [CANFAR Science Portal](https://www.canfar.net/), built with Rust, GTK 4, and libadwaita.

This is the Linux counterpart of [Verbinal for Windows](https://github.com/szautkin/CanfarDesktop) (C#/WinUI 3).

![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)

## Features

- **Session Management** - Launch, monitor, renew, and delete CANFAR science sessions (Notebook, Desktop, CARTA, Contributed, Firefly, Headless)
- **Storage Quota** - View VOSpace home directory usage at a glance
- **Platform Load** - Real-time cluster CPU, GPU, and RAM utilisation
- **Recent Launches** - Quick re-launch from session history
- **Standard & Advanced Launch** - Pick from the CANFAR image catalogue or supply a custom registry image with auth credentials
- **Auto-Refresh** - Active sessions poll automatically while any session is pending
- **Secure Credentials** - Tokens stored in the system keyring via Secret Service (GNOME Keyring / KDE Wallet)

## Screenshot

*(coming soon)*

## Requirements

### Runtime
- GTK 4.12+
- libadwaita 1.4+
- A Secret Service provider (GNOME Keyring, KDE Wallet, or similar)
- A CANFAR account

### Build
- Rust 1.75+ (2021 edition)
- System development packages:
  ```
  # Debian / Ubuntu
  sudo apt install libgtk-4-dev libadwaita-1-dev pkg-config

  # Fedora
  sudo dnf install gtk4-devel libadwaita-devel

  # Arch
  sudo pacman -S gtk4 libadwaita
  ```

## Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run
cargo run --release
```

The release binary will be at `target/release/canfar-ubuntu`.

## Running Tests

```bash
cargo test
```

## Code Quality

```bash
# Lint
cargo clippy -- -D warnings

# Format check
cargo fmt -- --check

# Format (apply)
cargo fmt
```

## Project Structure

```
src/
  main.rs              # Application entry point
  config.rs            # API endpoints and app configuration
  state.rs             # Shared application state (AppServices)
  style.css            # GTK CSS theme overrides
  helpers/             # Utility functions (image parsing)
  models/              # Data structures (Session, Image, etc.)
  services/            # API clients and business logic
  ui/                  # GTK widget components
assets/                # Session type icons
```

## Architecture

- **GTK 4 + libadwaita** for the UI layer
- **Tokio** multi-threaded runtime for async HTTP, bridged to GTK's GLib main loop via `oneshot` channels
- **Reqwest** with Rustls for HTTPS API calls
- **Rc/RefCell** ownership model for GTK widgets; `Arc` for cross-thread shared state
- Clean separation: Models -> Services -> UI

## API Endpoints

All communication is with CANFAR services over HTTPS. No telemetry, analytics, or third-party calls.

| Service | Base URL | Purpose |
|---------|----------|---------|
| Auth | `ws-cadc.canfar.net/ac` | Login, token validation, user info |
| Sessions | `ws-uv.canfar.net/skaha/v1` | Session CRUD, images, context, stats |
| Storage | `ws-uv.canfar.net/arc` | VOSpace quota |

## License

[GNU Affero General Public License v3.0](LICENSE)

Copyright (C) 2025 Serhii Zautkin

## Privacy

See [PRIVACY.md](PRIVACY.md). In short: no data collection, no telemetry, no third-party services. All data stays on your machine or goes directly to CANFAR.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
