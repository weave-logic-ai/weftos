#!/usr/bin/env bash
# One-click VPS deployment script for weft (clawft).
#
# Usage:
#   ./scripts/deploy/vps-deploy.sh [OPTIONS]
#
# Options:
#   --image IMAGE    Docker image to deploy (default: ghcr.io/weave-logic-ai/weftos:latest)
#   --port PORT      Host port for the gateway (default: 8080)
#   --name NAME      Container name (default: weft)
#   --config DIR     Host directory for config (default: ~/.clawft)
#   --restart        Restart policy (default: unless-stopped)
#   --pull           Pull latest image before deploying
#   --help           Show this help message
#
# Prerequisites:
#   - Docker installed and running
#   - User in docker group (or run with sudo)

set -euo pipefail

# Defaults
IMAGE="ghcr.io/weave-logic-ai/weftos:latest"
PORT="8080"
CONTAINER_NAME="weft"
CONFIG_DIR="$HOME/.clawft"
RESTART_POLICY="unless-stopped"
PULL=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --image)    IMAGE="$2";           shift 2 ;;
        --port)     PORT="$2";            shift 2 ;;
        --name)     CONTAINER_NAME="$2";  shift 2 ;;
        --config)   CONFIG_DIR="$2";      shift 2 ;;
        --restart)  RESTART_POLICY="$2";  shift 2 ;;
        --pull)     PULL=true;            shift   ;;
        --help)
            head -20 "$0" | tail -15
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

echo "=== weft VPS deployment ==="
echo "Image:     $IMAGE"
echo "Port:      $PORT"
echo "Container: $CONTAINER_NAME"
echo "Config:    $CONFIG_DIR"
echo "Restart:   $RESTART_POLICY"
echo ""

# Check Docker
if ! command -v docker &>/dev/null; then
    echo "ERROR: Docker is not installed. Install Docker first:" >&2
    echo "  curl -fsSL https://get.docker.com | sh" >&2
    exit 1
fi

# Ensure config directory exists
mkdir -p "$CONFIG_DIR"

# Pull latest image if requested
if [ "$PULL" = true ]; then
    echo "Pulling latest image..."
    docker pull "$IMAGE"
fi

# Stop existing container if running
if docker ps -a --format '{{.Names}}' | grep -qw "$CONTAINER_NAME"; then
    echo "Stopping existing container '$CONTAINER_NAME'..."
    docker stop "$CONTAINER_NAME" 2>/dev/null || true
    docker rm "$CONTAINER_NAME" 2>/dev/null || true
fi

# Run the container
echo "Starting weft gateway..."
docker run -d \
    --name "$CONTAINER_NAME" \
    --restart "$RESTART_POLICY" \
    -p "${PORT}:8080" \
    -v "${CONFIG_DIR}:/home/weft/.clawft" \
    "$IMAGE" gateway

echo ""
echo "weft gateway is running on port $PORT"
echo "  Container: $CONTAINER_NAME"
echo "  Config:    $CONFIG_DIR"
echo ""
echo "Useful commands:"
echo "  docker logs -f $CONTAINER_NAME    # View logs"
echo "  docker stop $CONTAINER_NAME       # Stop"
echo "  docker restart $CONTAINER_NAME    # Restart"
