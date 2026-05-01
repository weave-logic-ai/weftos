# clawft-ui — multi-stage Docker image (WEFT-317)
#
# Stage 1: build the React + Vite SPA under node:lts-alpine.
# Stage 2: serve the static dist/ via nginx:alpine. No Node.js in the
#          runtime image — keeps the surface ~25 MB and cuts CVEs.
#
# Build (run from the repo root so the build context contains clawft-ui/):
#
#   docker build -f clawft-ui/Dockerfile -t clawft-ui:0.7.0 .
#
# or via the wrapper:
#
#   scripts/build.sh ui-docker
#
# The compiled SPA expects to talk to an Axum gateway at the same origin.
# Use a sidecar / reverse proxy that forwards /api and /ws to the gateway,
# or pass VITE_API_URL=https://api.example.com as a build arg.

# ── Stage 1: build ───────────────────────────────────────────────────
FROM node:lts-alpine AS build

ARG VITE_API_URL=""
ARG VITE_BACKEND_MODE="axum"

ENV VITE_API_URL=${VITE_API_URL}
ENV VITE_BACKEND_MODE=${VITE_BACKEND_MODE}

WORKDIR /app

# Install dependencies first so the layer caches across source-only changes.
COPY clawft-ui/package.json clawft-ui/package-lock.json ./
RUN npm ci --no-audit --no-fund

# Copy the rest of the source and build the production bundle.
COPY clawft-ui/ ./
RUN npm run build && npm prune --production

# ── Stage 2: serve ───────────────────────────────────────────────────
FROM nginx:alpine

# Drop privileges where possible. nginx:alpine still needs root to bind 80
# and to write the access/error logs, so we keep the default user but
# disable the daemonize fork (the image's CMD already does this).
COPY --from=build /app/dist /usr/share/nginx/html
COPY clawft-ui/nginx.conf /etc/nginx/conf.d/default.conf

# Healthcheck: a 200 on / means the SPA index served (and so the WASM
# fallback / mime types are in play). curl is in alpine by default.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD wget -q --spider http://localhost/ || exit 1

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]
