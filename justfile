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

