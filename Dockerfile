FROM rust:1.72.0-slim-buster AS builder

COPY Cargo.toml ./

COPY src ./src

RUN cargo build --release
