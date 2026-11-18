default:
    @just --list

serve:
    cargo run --locked

open:
    cargo run --locked -- --open

start:
    cargo run --locked -- --start

test:
    cargo test --locked

# Build a release ZIP for this machine's platform.
dist:
    {{ if os() == "windows" { "python" } else { "python3" } }} scripts/dist.py

