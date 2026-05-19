## syntax=docker/dockerfile:1.7

FROM node:22-alpine AS frontend-deps
ARG TARGETARCH

WORKDIR /app/admin-ui
RUN corepack enable
COPY admin-ui/package.json admin-ui/pnpm-lock.yaml admin-ui/.npmrc admin-ui/pnpm-workspace.yaml ./
RUN --mount=type=cache,id=pnpm-store-${TARGETARCH},target=/pnpm/store \
    pnpm config set store-dir /pnpm/store && \
    pnpm install --frozen-lockfile

FROM frontend-deps AS frontend-builder
ARG TARGETARCH

COPY admin-ui ./
RUN --mount=type=cache,id=pnpm-store-${TARGETARCH},target=/pnpm/store \
    pnpm config set store-dir /pnpm/store && \
    pnpm build

FROM rust:1.92-alpine AS chef
ARG TARGETARCH

RUN apk add --no-cache musl-dev perl make
RUN --mount=type=cache,id=cargo-chef-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-chef-git-${TARGETARCH},target=/usr/local/cargo/git \
    cargo install cargo-chef --locked

WORKDIR /app

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG TARGETARCH

COPY --from=planner /app/recipe.json ./recipe.json
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git-${TARGETARCH},target=/usr/local/cargo/git \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/app/target \
    cargo chef cook --release --no-default-features --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=frontend-builder /app/admin-ui/dist /app/admin-ui/dist
RUN --mount=type=cache,id=cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git-${TARGETARCH},target=/usr/local/cargo/git \
    --mount=type=cache,id=cargo-target-${TARGETARCH},target=/app/target \
    cargo build --release --no-default-features && \
    cp /app/target/release/kiro-rs /app/kiro-rs

FROM alpine:3.21

RUN apk add --no-cache ca-certificates docker-cli docker-cli-compose

WORKDIR /app
COPY --from=builder /app/kiro-rs /app/kiro-rs

VOLUME ["/app/config"]

EXPOSE 8990

CMD ["./kiro-rs", "-c", "/app/config/config.json", "--credentials", "/app/config/credentials.json"]
