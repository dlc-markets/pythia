# See https://gist.github.com/noelbundick/6922d26667616e2ba5c3aff59f0824cd
FROM rust:1.73-slim-bookworm AS builder

RUN --mount=type=cache,target=/var/cache/apt \
    apt update -y && apt install -y \
    pkg-config \
    make \
    g++ \
    libssl-dev

WORKDIR /app

COPY Cargo.toml ./

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=pythia-registry \
    --mount=type=cache,target=/app/target,id=pythia \
    mkdir src && \
    touch src/lib.rs && \
    cargo build --release

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=pythia-registry \
    --mount=type=cache,target=/app/target,id=pythia \ <<EOF
  set -e
  touch /app/src/main.rs
  cargo build --release
  mv /app/target/release/pythia /app/pythia
EOF

FROM debian:bookworm-slim

COPY --from=builder /app/pythia /usr/bin/pythia

RUN apt update -y && apt install -y libssl-dev

RUN groupadd --gid 1000 pythia \
    && useradd --uid 1000 --gid 1000 -m pythia

WORKDIR /home/pythia

COPY config.json /home/pythia/config.json

COPY migrations ./migrations

USER pythia

EXPOSE 8000

CMD [ "pythia", "-c", "/home/pythia/config.json" ]
