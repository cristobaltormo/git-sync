FROM golang:1.26-alpine AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY cmd ./cmd
COPY internal ./internal
ARG VERSION=dev
RUN CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X main.version=${VERSION}" -o /gitsync ./cmd/gitsync

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
