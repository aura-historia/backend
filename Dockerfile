# The host image: every long-running process and one-shot command of the backend.
# Build with the repository root as context and --platform linux/amd64. Run one entrypoint:
#   api | mcp | worker | ops <command>
# Email templates: every mjml/<area>/<kind>/<language>.mjml, compiled with the pinned MJML to
# <area>/<kind>/<language>.html under MAIL_TEMPLATE_DIR.
FROM node:26.0.0-bookworm-slim@sha256:3529ef69feecddd94e9c5ecd3d25a96f2f23ea40661f494f932ae2b12ab1977c AS mail-templates
WORKDIR /build/mjml
COPY mjml/package.json mjml/package-lock.json ./
RUN npm ci --ignore-scripts --no-audit --no-fund
COPY mjml/ ./
RUN set -eu; count=0; \
    for template in $(find . -path ./node_modules -prune -o -name '*.mjml' -print | sort); do \
        output="/templates/${template#./}"; output="${output%.mjml}.html"; \
        mkdir -p "$(dirname "$output")"; \
        node_modules/.bin/mjml "$template" --output "$output"; \
        count=$((count + 1)); \
    done; \
    test "$count" -gt 0

FROM rust:1.98.0-bookworm@sha256:4e4a7e7939c17991ab35f2b8c2e67593980f771d28f6b1254b1850f860fd0c7f AS build
WORKDIR /build
COPY . .
RUN cargo build --locked --release \
        -p aura-historia-api --bin aura-historia-api \
        -p aura-historia-mcp --bin aura-historia-mcp \
        -p aura-historia-worker --bin aura-historia-worker \
        -p aura-historia-ops --bin aura-historia-ops && \
    test -s /etc/ssl/certs/ca-certificates.crt

FROM debian:bookworm-slim@sha256:f3034a6ec3c1205360777c4aae76234998866ad18806ae62b63a3f84ccad782b
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --gid 10001 aura && useradd --uid 10001 --gid 10001 --no-create-home aura && \
    chown 10001:10001 /tmp && chmod 0700 /tmp
COPY --from=build /build/target/release/aura-historia-api /usr/local/bin/api
COPY --from=build /build/target/release/aura-historia-mcp /usr/local/bin/mcp
COPY --from=build /build/target/release/aura-historia-worker /usr/local/bin/worker
COPY --from=build /build/target/release/aura-historia-ops /usr/local/bin/ops
COPY --from=mail-templates /templates /usr/share/aura-historia/mail-templates
ARG SOURCE_REVISION
ARG BUILD_WORKFLOW_IDENTITY
LABEL org.opencontainers.image.title="aura-historia" \
      org.opencontainers.image.source="https://github.com/aura-historia/backend" \
      org.opencontainers.image.revision="${SOURCE_REVISION}" \
      com.aura-historia.build-workflow="${BUILD_WORKFLOW_IDENTITY}"
ENV AURA_HISTORIA_SOURCE_REVISION="${SOURCE_REVISION}" \
    MAIL_TEMPLATE_DIR=/usr/share/aura-historia/mail-templates
USER 10001:10001
VOLUME ["/tmp"]
EXPOSE 8080
CMD ["api"]
