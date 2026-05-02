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
#
# `nginxinc/nginx-unprivileged` is the upstream Nginx image configured
# to run as the `nginx` user (uid 101) with logs going to stdout/stderr
# and the worker binding to port 8080 — so we never run as root inside
# the container, regardless of how the host orchestrator schedules it.
# WEFT-576 (security audit: don't run as root).
#
# The default upstream image listens on 8080 and we mirror that in
# `nginx.conf`. Operators map external ports as they wish:
#
#   docker run -p 80:8080 ghcr.io/.../clawft-ui:0.7.0   # public 80
#   docker run -p 8080:8080 ghcr.io/.../clawft-ui:0.7.0 # 1:1
FROM nginxinc/nginx-unprivileged:alpine

# Static SPA assets — `nginx-unprivileged` uses the same html dir.
COPY --from=build /app/dist /usr/share/nginx/html
COPY clawft-ui/nginx.conf /etc/nginx/conf.d/default.conf

# Healthcheck on the unprivileged port. wget is in the alpine base.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD wget -q --spider http://localhost:8080/ || exit 1

EXPOSE 8080

# `nginx-unprivileged`'s default CMD already runs `nginx -g daemon off;`
# as the non-root user. Inheriting it is correct.
