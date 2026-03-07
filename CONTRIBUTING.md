# Contributing to Verbinal for Linux

Thank you for your interest in contributing! This document provides guidelines
for contributing to the project.

## Getting Started

1. Fork the repository
2. Clone your fork and create a branch:
   ```bash
   git clone git@github.com:YOUR_USERNAME/CanfarDesktopUbuntu.git
   cd CanfarDesktopUbuntu
   git checkout -b my-feature
   ```
3. Install dependencies (see [README.md](README.md#build))
4. Build and test:
   ```bash
   cargo build
   cargo test
   cargo clippy -- -D warnings
   ```

## Code Style

- Run `cargo fmt` before committing
- Run `cargo clippy -- -D warnings` and fix all warnings
- Follow existing code patterns and naming conventions
- Keep changes focused — one feature or fix per PR

## Pull Requests

1. Ensure your code compiles without warnings (`cargo build`, `cargo clippy`)
2. Format your code (`cargo fmt`)
3. Run tests (`cargo test`)
4. Write a clear PR description explaining what and why
5. Reference any related issues

## Reporting Issues

- Use the GitHub issue tracker
- Include your Linux distribution and GTK/libadwaita versions
- Include steps to reproduce the problem
- Include any error output from the terminal

## License

By contributing, you agree that your contributions will be licensed under the
[AGPL-3.0 license](LICENSE).
