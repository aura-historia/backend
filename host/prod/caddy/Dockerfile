# Caddy with the Cloudflare DNS provider, so certificates for the CloudFront-fronted names are
# issued through the DNS challenge and port 80 stays closed. Build context: host/prod/caddy.
FROM caddy:2.11.7-builder@sha256:c4cd20eaa0b69112fd5d4f721ffc9671687e1d5d0b155c8620cf7546dbef94c0 AS build
RUN xcaddy build v2.11.7 --with github.com/caddy-dns/cloudflare@v0.2.4

FROM caddy:2.11.7@sha256:36c24ad91a7ff7bc4c87fe22bfa8df3c4b18da0773d73fc12ee2c52b98c0bb7e
COPY --from=build /usr/bin/caddy /usr/bin/caddy
