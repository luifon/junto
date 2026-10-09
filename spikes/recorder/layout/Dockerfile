FROM nginx:alpine
ARG LIVEKIT_CLIENT_VERSION=2.22.3
ADD https://cdn.jsdelivr.net/npm/livekit-client@${LIVEKIT_CLIENT_VERSION}/dist/livekit-client.umd.js /usr/share/nginx/html/livekit-client.umd.js
RUN chmod 644 /usr/share/nginx/html/livekit-client.umd.js
COPY index.html layout.js /usr/share/nginx/html/
