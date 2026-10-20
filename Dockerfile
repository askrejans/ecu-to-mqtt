# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:1-slim-bookworm AS builder

# cmake + perl for aws-lc-sys (rustls crypto provider)
RUN apt-get update && apt-get install -y --no-install-recommends \
        perl make cmake pkg-config \
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
    && useradd -r -g ecu-to-mqtt --groups dialout,tty --no-create-home \
        --shell /usr/sbin/nologin ecu-to-mqtt \
    && mkdir -p /etc/ecu-to-mqtt /var/lib/ecu-to-mqtt \
    && chown ecu-to-mqtt:ecu-to-mqtt /var/lib/ecu-to-mqtt

COPY --from=builder /app/target/release/ecu-to-mqtt /usr/local/bin/ecu-to-mqtt
COPY example.settings.toml /etc/ecu-to-mqtt/settings.toml
COPY LICENSE /usr/share/doc/ecu-to-mqtt/copyright

USER ecu-to-mqtt
WORKDIR /var/lib/ecu-to-mqtt

# HTTP API on by default so the container health probe works; loopback only
# unless ECU_TO_MQTT_HTTP_BIND=0.0.0.0 (then also set ECU_TO_MQTT_HTTP_TOKEN).
ENV ECU_TO_MQTT_HTTP_ENABLED=true \
    ECU_TO_MQTT_HTTP_BIND=127.0.0.1

# HTTP API, web dashboard and Prometheus metrics
EXPOSE 9091

# Healthy = ECU link up and fresh channels (HTTP 200 from /health).
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["ecu-to-mqtt", "--healthcheck"]

# Configure with ECU_TO_MQTT_* environment variables or mount a settings.toml.
ENTRYPOINT ["ecu-to-mqtt", "--headless"]
CMD ["--config", "/etc/ecu-to-mqtt/settings.toml"]
