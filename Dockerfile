FROM rust:1.72.0-slim-bookworm AS builder

ENV SQLX_OFFLINE=true

WORKDIR /usr/src/pythia

RUN apt-get update -y && \
  apt-get install -y pkg-config make g++ libssl-dev

COPY Cargo.toml ./

COPY src ./src

COPY .sqlx ./.sqlx

COPY migrations ./migrations

RUN cargo build --release

FROM debian:bookworm-slim

ENV RUST_LOG info

RUN apt-get update -y && apt-get install -y libssl-dev

RUN groupadd --gid 1000 pythia \
    && useradd --uid 1000 --gid 1000 -m pythia

COPY --from=builder /usr/src/pythia/target/release/pythia /usr/bin/pythia

COPY config ./config

USER pythia

EXPOSE 8000

CMD [ "pythia" ]
