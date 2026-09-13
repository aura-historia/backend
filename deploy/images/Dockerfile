FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS build

# Signed, fixed Debian snapshot keeps native dependency resolution stable too.
RUN rm /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260901T000000Z bookworm main' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260901T000000Z bookworm-security main' \
        > /etc/apt/sources.list \
    && apt-get update \
    && apt-get install --yes --no-install-recommends cmake pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Use the installed pin; do not download the repo's editor-only components.
ENV RUSTUP_TOOLCHAIN=1.98.0 \
    CARGO_BUILD_JOBS=2
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo/config.toml .cargo/config.toml
COPY src/ src/
COPY migrations/ migrations/
COPY opensearch/ opensearch/

ARG COMMIT_SHA
RUN test "${#COMMIT_SHA}" -eq 40 \
    && printf '%s\n' "${COMMIT_SHA}" | grep -Eq '^[0-9a-f]{40}$' \
    && ! printf '%s\n' "${COMMIT_SHA}" | grep -Eq '^([0-9a-f])\1{39}$|^0123456789abcdef' \
    && cargo build --release --locked \
        -p aura-historia-api --bin aura-historia-api \
        -p aura-historia-worker --bin aura-historia-worker \
        -p aura-historia-cron --bin aura-historia-cron \
        -p crawler --bin server

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime
RUN rm /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20260901T000000Z bookworm main' \
        'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20260901T000000Z bookworm-security main' \
        > /etc/apt/sources.list \
    && apt-get update \
    && apt-get install --yes --no-install-recommends \
        ca-certificates libssl3 libgcc-s1 libstdc++6 zlib1g \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 aura \
    && useradd --uid 10001 --gid 10001 --create-home --shell /usr/sbin/nologin aura
ARG COMMIT_SHA
ENV COMMIT_SHA=${COMMIT_SHA} \
    HOME=/home/aura
LABEL org.opencontainers.image.source="https://github.com/aura-historia/backend" \
      org.opencontainers.image.revision="${COMMIT_SHA}"
WORKDIR /home/aura
USER 10001:10001
STOPSIGNAL SIGTERM

FROM runtime AS api
COPY --from=build /app/target/release/aura-historia-api /usr/local/bin/aura-historia-api
ENTRYPOINT ["/usr/local/bin/aura-historia-api"]

FROM runtime AS worker
COPY --from=build /app/target/release/aura-historia-worker /usr/local/bin/aura-historia-worker
ENTRYPOINT ["/usr/local/bin/aura-historia-worker"]

FROM runtime AS cron
COPY --from=build /app/target/release/aura-historia-cron /usr/local/bin/aura-historia-cron
ENTRYPOINT ["/usr/local/bin/aura-historia-cron"]

FROM runtime AS crawler
COPY --from=build /app/target/release/server /usr/local/bin/server
ENTRYPOINT ["/usr/local/bin/server"]

# Isolated smoke tooling only; never included in the four application images.
FROM build AS smoke-build
RUN cargo build --release --locked -p crawler --bin bootstrap-local

FROM runtime AS smoke-helper
USER root
RUN apt-get update \
    && apt-get install --yes --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=smoke-build /app/target/release/bootstrap-local /usr/local/bin/bootstrap-local
USER 10001:10001
ENTRYPOINT ["/usr/bin/python3"]
