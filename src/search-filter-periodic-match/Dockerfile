# Build with the repository root as context and --platform linux/amd64.
FROM rust:1.98.0-bookworm@sha256:4e4a7e7939c17991ab35f2b8c2e67593980f771d28f6b1254b1850f860fd0c7f AS build
WORKDIR /build
COPY . .
RUN cargo build --locked --release -p search-filter-periodic-match --bin search-filter-periodic-match && \
    ldd target/release/search-filter-periodic-match && \
    test -s /etc/ssl/certs/ca-certificates.crt

FROM debian:bookworm-slim@sha256:f3034a6ec3c1205360777c4aae76234998866ad18806ae62b63a3f84ccad782b
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 matcher && useradd --uid 10001 --gid 10001 --no-create-home matcher && \
    chown 10001:10001 /tmp && chmod 0700 /tmp && \
    mkdir -p /opt/aura-historia/rds-ca
COPY --from=build /build/target/release/search-filter-periodic-match /usr/local/bin/search-filter-periodic-match
COPY infra/assets/rds-ca-layer/aura-historia/rds-ca/global-bundle.pem /opt/aura-historia/rds-ca/global-bundle.pem
ARG SOURCE_REVISION
ARG BUILD_WORKFLOW_IDENTITY
LABEL org.opencontainers.image.title="search-filter-periodic-match" \
      org.opencontainers.image.source="https://github.com/aura-historia/backend" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      com.aura-historia.build-workflow="${BUILD_WORKFLOW_IDENTITY}"
ENV POSTGRES_TLS_ROOT_CERT=/opt/aura-historia/rds-ca/global-bundle.pem
USER 10001:10001
VOLUME ["/tmp"]
ENTRYPOINT ["/usr/local/bin/search-filter-periodic-match"]
CMD []
