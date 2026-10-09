# syntax=docker/dockerfile:1
# fleet-updater: the hub's update sidecar (docs/updates.md → fleet-updater).
# Built and published beside fleet-hub by .github/workflows/hub-image.yml.
FROM rust:1.99.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
# Only the manifests of the other members matter here (workspace
# resolution); nothing but fleet-update and fleet-updater is compiled.
COPY crates ./crates
COPY src-tauri/Cargo.toml ./src-tauri/Cargo.toml
RUN mkdir -p src-tauri/src && echo '' > src-tauri/src/lib.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build -p fleet-updater --release --locked \
 && cp target/release/fleet-updater /fleet-updater

FROM debian:bookworm-slim
# ca-certificates: `--standalone` reads the release channel over HTTPS.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /var/lib/fleet-updater /hub-data
COPY --from=build /fleet-updater /usr/local/bin/fleet-updater
# Root on purpose: it talks to the Docker socket (root-equivalent already)
# and gives a restored database back to the hub's own uid.
ENV FLEET_UPDATER_STATE_DIR=/var/lib/fleet-updater FLEET_UPDATER_HUB_DATA=/hub-data
VOLUME ["/var/lib/fleet-updater"]
ENTRYPOINT ["tini", "--", "fleet-updater"]
CMD ["run"]
