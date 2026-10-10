FROM alpine:3.22
RUN apk add --no-cache wireguard-go wireguard-tools-wg socat iproute2
COPY entrypoint.sh /entrypoint.sh
RUN chmod 755 /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
