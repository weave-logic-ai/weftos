# Deployment Guide

> Deploying the ClawFT UI in various configurations.

## Overview

The ClawFT UI can be deployed in several ways:

| Mode | Backend | Static Hosting | Server Required |
|------|---------|---------------|-----------------|
| Docker | Axum | nginx container | Yes (Axum container) |
| CDN | Axum | S3/Cloudflare/Vercel | Yes (separate Axum server) |
| Reverse Proxy | Axum | nginx/Caddy in front | Yes |
| Single Binary | Embedded | rust-embed | No separate UI server |
| Tauri | Axum or WASM | Desktop app | Optional |
| Browser-Only | WASM | Any static host | No |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `VITE_API_URL` | `""` | Axum backend URL (empty = same origin) |
| `VITE_BACKEND_MODE` | `"axum"` | `axum`, `wasm`, `auto`, `mock` |
| `VITE_MOCK_API` | `"false"` | Enable MSW mocks for development |

These are build-time variables (Vite inlines them during `npm run build`).

## Docker Deployment

### UI Container (nginx)

```dockerfile
# ui/Dockerfile
FROM node:20-alpine AS build
WORKDIR /app
COPY ui/package*.json ./
RUN npm ci
COPY ui/ ./
RUN npm run build

FROM nginx:alpine
COPY --from=build /app/dist /usr/share/nginx/html
COPY ui/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
```

### nginx Configuration

```nginx
# ui/nginx.conf
server {
    listen 80;
    server_name _;
    root /usr/share/nginx/html;
    index index.html;

    # SPA fallback
    location / {
        try_files $uri $uri/ /index.html;
    }

    # WASM content type
    types {
        application/wasm wasm;
    }

    # Cache static assets
    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    # Gzip
    gzip on;
    gzip_types text/plain text/css application/json application/javascript application/wasm;
}
```

### Docker Compose

```yaml
# docker-compose.yml
services:
  backend:
    build:
      context: .
      dockerfile: Dockerfile
    ports:
      - "18789:18789"
    environment:
      - RUST_LOG=info

  ui:
    build:
      context: .
      dockerfile: ui/Dockerfile
      args:
        VITE_API_URL: http://backend:18789
    ports:
      - "80:80"
    depends_on:
      - backend
```

## CDN Deployment

Build the UI and upload the `dist/` directory to any static hosting service.

```bash
cd ui
npm run build

# S3
aws s3 sync dist/ s3://my-clawft-bucket --delete

# Cloudflare Pages
npx wrangler pages deploy dist/

# Vercel
npx vercel deploy dist/
```

Configure the CDN to:
- Serve `index.html` for all routes (SPA fallback)
- Set `Content-Type: application/wasm` for `.wasm` files
- Enable gzip/brotli compression

The UI must be configured to point to the Axum backend:

```bash
VITE_API_URL=https://api.my-clawft.example.com npm run build
```

## Reverse Proxy

### nginx

```nginx
server {
    listen 443 ssl http2;
    server_name clawft.example.com;

    ssl_certificate /etc/ssl/certs/clawft.pem;
    ssl_certificate_key /etc/ssl/private/clawft.key;

    # UI static files
    location / {
        root /var/www/clawft/dist;
        try_files $uri $uri/ /index.html;
    }

    # API proxy
    location /api/ {
        proxy_pass http://127.0.0.1:18789;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # WebSocket proxy
    location /ws {
        proxy_pass http://127.0.0.1:18789;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 86400;
    }

    # WASM content type
    types {
        application/wasm wasm;
    }
}
```

### Caddy

```
clawft.example.com {
    # UI static files
    root * /var/www/clawft/dist
    file_server
    try_files {path} /index.html

    # API proxy
    reverse_proxy /api/* localhost:18789

    # WebSocket proxy
    reverse_proxy /ws localhost:18789
}
```

## Single Binary (rust-embed)

The `weft ui` command serves the UI from a binary with embedded static files using `rust-embed`. No separate web server needed.

```bash
# Build the UI first
cd clawft-ui && npm run build && cd ..

# Build the Rust binary (includes dist/ via rust-embed)
cargo build --release --bin weft

# Run
./target/release/weft serve --port 18789
```

The binary serves both the API and UI on the same port.

## Tauri Packaging

Tauri wraps the web UI in a native desktop application.

```bash
# Install Tauri CLI
cargo install tauri-cli

# Initialize Tauri project (one-time)
cd ui
cargo tauri init

# Build for current platform
cargo tauri build
```

Output locations:
- **macOS:** `src-tauri/target/release/bundle/dmg/`
- **Linux:** `src-tauri/target/release/bundle/deb/` and `appimage/`
- **Windows:** `src-tauri/target/release/bundle/msi/`

Tauri can use either Axum mode (connecting to an external server) or WASM mode (self-contained).

## Browser-Only Mode

Deploy a completely self-contained UI with no backend. Users configure their LLM provider directly in the browser.

### Build

```bash
cd ui

# Build with WASM mode
VITE_BACKEND_MODE=wasm npm run build

# Copy WASM files
cp public/clawft_wasm.js dist/
cp public/clawft_wasm_bg.wasm dist/
```

### Required Headers

The static server must set these headers for WASM to work:

```
Content-Type: application/wasm  (for .wasm files)
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

### URL-Based Mode Switching

Users can switch modes at runtime via URL parameters:

```
https://app.example.com/?mode=axum    # Server mode
https://app.example.com/?mode=wasm    # Browser mode
https://app.example.com/?mode=auto    # Auto-detect
```

See the [Browser Mode Guide](browser-mode.md) for provider setup, CORS configuration, and troubleshooting.
