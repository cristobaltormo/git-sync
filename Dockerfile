FROM rust:1-alpine AS build
RUN apk add --no-cache gcc musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
ARG VERSION=dev
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    GITSYNC_VERSION=${VERSION} cargo build --release --locked \
 && cp target/release/gitsync /gitsync

FROM alpine:3.22
RUN apk add --no-cache git ca-certificates \
 && adduser -D -u 10001 gitsync \
 && mkdir /data /config \
 && chown gitsync /data
COPY --from=build /gitsync /usr/local/bin/gitsync
USER gitsync
ENV GITSYNC_CONFIG=/config/config.toml STATE_DIRECTORY=/data
VOLUME /data
EXPOSE 9001
ENTRYPOINT ["gitsync"]
CMD ["run"]
