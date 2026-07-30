default:
    @just --list

build:
    cargo build --manifest-path Cargo.toml --locked --workspace

mcp *args:
    cargo run --manifest-path Cargo.toml --locked -p s2-mcp -- {{args}}

check:
    cargo check --manifest-path Cargo.toml --locked --workspace --all-targets

clippy:
    cargo clippy --manifest-path Cargo.toml --locked --workspace --all-features --all-targets -- -D warnings

deny:
    cargo deny --manifest-path Cargo.toml --config deny.toml --workspace --locked check

fmt:
    cargo +nightly fmt --manifest-path Cargo.toml --all

test:
    cargo test --manifest-path Cargo.toml --locked --workspace --all-features
