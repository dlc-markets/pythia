FROM rust:1.72.0-slim-bookworm AS builder

WORKDIR /usr

COPY Cargo.toml ./

COPY src ./src

COPY .docker/ ./

RUN apt-get update -y && \
  apt-get install -y pkg-config make g++ libssl-dev

ENV SQLX_OFFLINE true

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update -y && apt-get install -y libssl-dev

COPY --from=builder /usr/target/release/pythia /usr/bin/pythia

COPY config ./config

ENV RUST_LOG info

CMD [ "/usr/bin/pythia" ]