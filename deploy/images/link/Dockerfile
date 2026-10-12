# The private link of the two-host form (ADR 0003): WireGuard in userspace and TCP
# forwarders, in one container on each host. Build from deploy/images/link.
ARG ALPINE_IMAGE=alpine:3.22.6@sha256:5291449c3df73caf6ed85e649dec1b9e818b39a5d8c871e97afc13e9cd5e8fa8
FROM ${ALPINE_IMAGE}
# Alpine keeps one version of a package per release. When a pinned version is replaced, the
# build fails here and the pin is updated on purpose.
ARG WIREGUARD_GO_VERSION=0.0.20250515-r5
ARG WIREGUARD_TOOLS_VERSION=1.0.20250521-r0
ARG SOCAT_VERSION=1.8.1.3-r0
ARG IPROUTE2_VERSION=6.15.0-r0
RUN apk add --no-cache \
      "wireguard-go=${WIREGUARD_GO_VERSION}" \
      "wireguard-tools-wg=${WIREGUARD_TOOLS_VERSION}" \
      "socat=${SOCAT_VERSION}" \
      "iproute2-minimal=${IPROUTE2_VERSION}"
COPY --chmod=0755 entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
