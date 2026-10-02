FROM rust:1.94-bookworm AS build
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo test --locked && cargo build --locked --release && cp target/release/pao-reports-web-rust-sample /tmp/reports-rust

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10001 --home /nonexistent --shell /usr/sbin/nologin reports
WORKDIR /opt/reports
COPY --from=build /tmp/reports-rust ./reports-rust
COPY public ./public
COPY resources ./resources
USER reports
ENV HOST=0.0.0.0 PORT=8080 REPORTS_RESOURCE_ROOT=/opt/reports/resources REPORTS_TEMPLATE=/opt/reports/public/index.html REPORTS_WEB_ROOT=/opt/reports/web
EXPOSE 8080
ENTRYPOINT ["/opt/reports/reports-rust"]
