# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder

# Build dependencies: perl + cmake for openssl-src, pkg-config for linkage
RUN apt-get update && apt-get install -y --no-install-recommends \
        perl \
        make \
        cmake \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies: copy manifests first, build a dummy main, then replace
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release --locked 2>/dev/null || true \
    && rm -rf src

# Build the real binary. The copied sources keep their host mtimes, which can be
# older than the dummy build above – without touching them cargo considers the
# dummy artifact fresh and the image ships a binary that does nothing.
COPY . .
RUN find src -type f -exec touch {} + \
    && cargo build --release --locked

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r ecu-to-mqtt \
    && useradd -r -g ecu-to-mqtt \
        --groups dialout,tty \
        --no-create-home \
        --shell /usr/sbin/nologin \
        ecu-to-mqtt \
    && mkdir -p /etc/ecu-to-mqtt

COPY --from=builder /app/target/release/ecu-to-mqtt /usr/local/bin/ecu-to-mqtt
COPY example.settings.toml /etc/ecu-to-mqtt/settings.toml

USER ecu-to-mqtt
WORKDIR /tmp

# Force service/log mode – no TUI in a container.
# Override with ECU_TO_MQTT_NO_TUI=0 if you run with `docker run -it`.
ENV ECU_TO_MQTT_NO_TUI=1

# All configuration is driven by environment variables (ECU_TO_MQTT_* prefix)
# or by mounting a settings.toml over /etc/ecu-to-mqtt/settings.toml.
CMD ["ecu-to-mqtt", "--config", "/etc/ecu-to-mqtt/settings.toml"]
