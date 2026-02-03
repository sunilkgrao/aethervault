# syntax=docker/dockerfile:1.7
FROM rust:1.76-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --locked --release

FROM debian:bookworm-slim
ARG WITH_PYTHON=0
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && if [ "$WITH_PYTHON" = "1" ]; then apt-get install -y --no-install-recommends python3; fi \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/aethervault /usr/local/bin/aethervault
COPY hooks /app/hooks
WORKDIR /app
ENTRYPOINT ["aethervault"]
