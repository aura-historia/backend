# The business PostgreSQL of the prod Compose project, with pgBackRest for WAL archiving and
# backups to S3. Build context: host/prod/postgres. See host/prod/README.md.
FROM postgres:16.15-bookworm@sha256:a49f7438ba03589f9b1ca062f2da82e899b48ea1dc50d2449fa09a70be83ebca
ARG PGBACKREST_VERSION=2.60.1-1.pgdg12+1
RUN apt-get update && \
    apt-get install -y --no-install-recommends "pgbackrest=${PGBACKREST_VERSION}" && \
    rm -rf /var/lib/apt/lists/* /etc/pgbackrest.conf && \
    install -d -m 0755 /etc/pgbackrest && \
    install -d -o postgres -g postgres -m 0750 /var/spool/pgbackrest /var/log/pgbackrest /tmp/pgbackrest
COPY --chmod=0644 postgresql.conf pg_hba.conf /etc/postgresql/
COPY --chmod=0644 pgbackrest.conf /etc/pgbackrest/pgbackrest.conf
COPY --chmod=0755 restore-check.sh /usr/local/bin/aura-restore-check
COPY --chmod=0755 initdb/ /docker-entrypoint-initdb.d/
CMD ["postgres", "-c", "config_file=/etc/postgresql/postgresql.conf"]
