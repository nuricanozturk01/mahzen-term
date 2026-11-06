# MahzenTerm

*A terminal with depth.*

Fast, modular, native GPU terminal and workspace (Linux, macOS, Windows), written in Rust.

- Product and architecture: [PLAN.md](PLAN.md)
- Contributor rules: [CLAUDE.md](CLAUDE.md)

## Quality gate
```
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo xtask check-deps
```

## License
MIT OR Apache-2.0
