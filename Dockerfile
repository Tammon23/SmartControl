FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY server ./server
COPY automation ./automation
COPY windows-tray ./windows-tray
RUN cargo build --locked --release -p bulb-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home bridge && mkdir -p /app/data && chown bridge:bridge /app/data
WORKDIR /app
COPY --from=build /source/target/release/bulb-server /usr/local/bin/bulb-server
USER bridge
ENV BULB_BIND=0.0.0.0:8787
EXPOSE 8787
ENTRYPOINT ["bulb-server"]
CMD ["/app/config.json"]
