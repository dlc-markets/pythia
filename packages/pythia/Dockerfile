FROM lukemathwalker/cargo-chef:latest-rust-latest AS chef

WORKDIR /app

FROM chef AS planner

COPY . .

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /app/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

COPY . .

ENV SQLX_OFFLINE=true

RUN cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update -y && apt-get install -y \
    libssl-dev \
    ca-certificates \
    netcat-traditional

RUN groupadd --gid 1000 pythia \
    && useradd --uid 1000 --gid 1000 -m pythia

WORKDIR /home/pythia

USER pythia

COPY packages/pythia/migrations ./migrations

COPY --from=builder /app/target/release/pythia /usr/local/bin/pythia

EXPOSE 8000

HEALTHCHECK --interval=30s --timeout=15s --start-period=5s --retries=3 \
    CMD [ "nc", "-zv", "localhost", "8000" ]

CMD [ "pythia" ]

