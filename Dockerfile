# syntax=docker/dockerfile:1
ARG RUST_VERSION=1.96.0
FROM rust:${RUST_VERSION}-bookworm AS build
ARG TARGETARCH
WORKDIR /source
COPY rust/Cargo.toml rust/Cargo.lock rust/build.rs ./rust/
COPY rust/src ./rust/src
COPY rust/schema ./rust/schema
COPY rust/data ./rust/data
COPY rust/templates ./rust/templates
COPY rust/static ./rust/static
COPY rust/vendor ./rust/vendor
COPY app/assets/stylesheets ./app/assets/stylesheets
COPY app/javascript ./app/javascript
COPY vendor/javascript ./vendor/javascript
COPY public ./public
RUN --mount=type=cache,id=telesink-rust-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=telesink-rust-target-${TARGETARCH},sharing=locked,target=/source/rust/target \
    CARGO_INCREMENTAL=0 cargo build --manifest-path rust/Cargo.toml --locked --release --jobs 2 --bin feeds && \
    install -D rust/target/release/feeds /out/feeds && install -d -m 0750 /out/data && ldd /out/feeds

FROM build AS migration-build
RUN --mount=type=cache,id=telesink-rust-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=telesink-rust-target-${TARGETARCH},sharing=locked,target=/source/rust/target \
    CARGO_INCREMENTAL=0 cargo build --manifest-path rust/Cargo.toml --locked --release --jobs 2 --features postgres-import --bin feeds-import-postgres && \
    install -D rust/target/release/feeds-import-postgres /out/feeds-import-postgres

FROM gcr.io/distroless/cc-debian12:nonroot AS migration
COPY --from=migration-build /out/feeds-import-postgres /feeds-import-postgres
COPY --chown=65532:65532 --from=build /out/data /data
COPY LICENSE /usr/share/licenses/feeds/LICENSE
USER 65532:65532
VOLUME ["/data"]
ENTRYPOINT ["/feeds-import-postgres"]
CMD ["--help"]

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=build /out/feeds /feeds
COPY --chown=65532:65532 --from=build /out/data /data
COPY LICENSE /usr/share/licenses/feeds/LICENSE
COPY rust/vendor/hotwired/*-LICENSE /usr/share/licenses/feeds/
USER 65532:65532
# Keep the legacy default overridable; explicit FEEDS_BIND takes precedence.
ENV TELESINK_BIND=0.0.0.0:3000
ENV DATABASE_URL=sqlite:///data/feeds.sqlite3
VOLUME ["/data"]
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s CMD ["/feeds", "healthcheck"]
ENTRYPOINT ["/feeds"]
CMD ["serve"]
