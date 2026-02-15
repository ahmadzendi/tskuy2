# ============ STAGE 1: Build ============
FROM rust:1.87-alpine AS builder

RUN apk add --no-cache \
    musl-dev \
    pkgconfig \
    openssl-dev \
    openssl-libs-static \
    perl \
    make

WORKDIR /app

# Cache dependencies layer
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Build actual app
COPY src/ src/
RUN touch src/main.rs && cargo build --release

# ============ STAGE 2: Runtime (minimal) ============
FROM alpine:3.20

RUN apk add --no-cache ca-certificates && \
    addgroup -S app && adduser -S app -G app

COPY --from=builder /app/target/release/gold-monitor /usr/local/bin/gold-monitor

USER app
ENV RUST_LOG=info
EXPOSE 10000

CMD ["gold-monitor"]
