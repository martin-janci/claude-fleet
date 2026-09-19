# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
# The workspace lists src-tauri; give cargo an empty lib so resolution
# succeeds without building the desktop crate.
RUN mkdir -p src-tauri/src && echo '' > src-tauri/src/lib.rs
COPY skills ./skills
COPY src/lib/names.json ./src/lib/names.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build -p fleet-hub --release --locked \
 && cp target/release/fleet-hub /fleet-hub

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-client git ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --create-home --shell /usr/sbin/nologin fleet \
 && mkdir -p /var/lib/fleet-hub && chown fleet:fleet /var/lib/fleet-hub
COPY --from=build /fleet-hub /usr/local/bin/fleet-hub
USER fleet
ENV FLEET_HUB_DATA_DIR=/var/lib/fleet-hub
VOLUME ["/var/lib/fleet-hub", "/home/fleet/.ssh"]
EXPOSE 4180
# Probes 127.0.0.1:$FLEET_HUB_PORT (default 4180) without opening state.db;
# only a response with the "fleet-hub ok" body counts as healthy (an
# unrelated 200 from another listener does not). It also reads
# $FLEET_HUB_TLS, so a hub that terminates TLS itself is probed over TLS.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["fleet-hub", "healthcheck"]
ENTRYPOINT ["tini", "--", "fleet-hub"]
CMD ["serve"]
