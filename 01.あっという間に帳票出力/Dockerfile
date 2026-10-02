FROM rust:1.94-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release
FROM alpine:3.22
WORKDIR /app
COPY --from=build /src/target/release/pao-reports-web-quick-start /app/quick-start
COPY index.html /app/index.html
COPY definition /app/definition
ENTRYPOINT ["/app/quick-start"]
