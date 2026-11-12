#!/usr/bin/env bash
# ==============================================================================
# AgentAudit — Local Development Environment Startup Script
# ==============================================================================
# Starts both the Rust/Axum backend and the React/Vite frontend concurrently.
# Optionally starts a Cloudflare Quick Tunnel for PayPal webhook testing.
#
# Usage:
#   ./scripts/dev.sh           # Local development (Backend + Frontend)
#   ./scripts/dev.sh --tunnel  # Local development + Cloudflare Quick Tunnel
# ==============================================================================

set -euo pipefail

# Determine repository root directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

ENABLE_TUNNEL=false
if [[ "${1:-}" == "--tunnel" ]]; then
  ENABLE_TUNNEL=true
elif [[ -n "${1:-}" ]]; then
  echo "Unknown option: $1"
  echo "Usage: $0 [--tunnel]"
  exit 1
fi

# Track child PIDs for clean termination
BACKEND_PID=""
FRONTEND_PID=""
TUNNEL_PID=""
TUNNEL_LOG=""

cleanup() {
  echo ""
  echo ">>> Shutting down AgentAudit development services..."

  if [[ -n "${TUNNEL_PID}" ]] && kill -0 "${TUNNEL_PID}" 2>/dev/null; then
    echo "Stopping Cloudflare Tunnel (PID: ${TUNNEL_PID})..."
    kill -TERM "${TUNNEL_PID}" 2>/dev/null || true
  fi

  if [[ -n "${FRONTEND_PID}" ]] && kill -0 "${FRONTEND_PID}" 2>/dev/null; then
    echo "Stopping React/Vite Frontend (PID: ${FRONTEND_PID})..."
    kill -TERM "${FRONTEND_PID}" 2>/dev/null || true
  fi

  if [[ -n "${BACKEND_PID}" ]] && kill -0 "${BACKEND_PID}" 2>/dev/null; then
    echo "Stopping Rust/Axum Backend (PID: ${BACKEND_PID})..."
    kill -TERM "${BACKEND_PID}" 2>/dev/null || true
  fi

  if [[ -n "${TUNNEL_LOG}" ]] && [[ -f "${TUNNEL_LOG}" ]]; then
    rm -f "${TUNNEL_LOG}"
  fi

  echo ">>> All AgentAudit processes stopped."
}

trap cleanup EXIT INT TERM

echo "======================================================================"
echo "                   AgentAudit Development Setup                       "
echo "======================================================================"

# 1. Verify required toolchain binaries
echo "[1/4] Verifying prerequisite CLI tools..."
for cmd in cargo node npm; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "ERROR: Required command '$cmd' is not installed or not in PATH."
    exit 1
  fi
done
echo "      Found: cargo ($(cargo --version | head -n1 | cut -d' ' -f1-2))"
echo "      Found: node ($(node --version))"
echo "      Found: npm ($(npm --version))"

if [[ "$ENABLE_TUNNEL" == true ]]; then
  if ! command -v cloudflared >/dev/null 2>&1; then
    echo "ERROR: --tunnel was requested, but 'cloudflared' was not found in PATH."
    echo "Install cloudflared from https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
    exit 1
  fi
  echo "      Found: cloudflared ($(cloudflared --version 2>&1 | head -n1))"
fi

# 2. Verify workspace structure & configuration
echo "[2/4] Verifying project paths and configuration..."
if [[ ! -f "${ROOT_DIR}/Cargo.toml" ]]; then
  echo "ERROR: Cargo.toml not found at ${ROOT_DIR}/Cargo.toml."
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/frontend/package.json" ]]; then
  echo "ERROR: frontend/package.json not found at ${ROOT_DIR}/frontend/package.json."
  exit 1
fi

if [[ ! -f "${ROOT_DIR}/.env" ]]; then
  if [[ -f "${ROOT_DIR}/.env.example" ]]; then
    echo "      Notice: .env not found. Creating .env from .env.example..."
    cp "${ROOT_DIR}/.env.example" "${ROOT_DIR}/.env"
    echo "      Created .env with default values. Please configure PAYPAL_CLIENT_ID / PAYPAL_CLIENT_SECRET for live testing."
  else
    echo "WARNING: Neither .env nor .env.example found. The backend will use defaults."
  fi
fi

# Ensure frontend dependencies are installed
if [[ ! -d "${ROOT_DIR}/frontend/node_modules" ]]; then
  echo "      Frontend dependencies (node_modules) not found. Running 'npm install' in frontend/..."
  npm --prefix "${ROOT_DIR}/frontend" install
fi

# 3. Start Backend
echo "[3/4] Launching Rust/Axum backend on port 5000..."
cd "${ROOT_DIR}"
cargo run &
BACKEND_PID=$!

# Wait for backend HTTP port to become responsive (timeout: 30s)
echo "      Waiting for backend to bind to http://127.0.0.1:5000/health..."
BACKEND_READY=false
for i in {1..30}; do
  if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
    echo "ERROR: Backend process exited unexpectedly during compilation or startup."
    exit 1
  fi

  if command -v curl >/dev/null 2>&1; then
    if curl -s -f http://127.0.0.1:5000/health >/dev/null 2>&1; then
      BACKEND_READY=true
      break
    fi
  else
    # Fallback to bash TCP connect if curl is unavailable
    if (echo > /dev/tcp/127.0.0.1/5000) >/dev/null 2>&1; then
      BACKEND_READY=true
      break
    fi
  fi
  sleep 1
done

if [[ "$BACKEND_READY" == true ]]; then
  echo "      Backend is healthy and listening on http://127.0.0.1:5000."
else
  echo "      Backend compilation still in progress or slow to bind; continuing..."
fi

# 4. Start Cloudflare Quick Tunnel (optional)
TUNNEL_URL=""
if [[ "$ENABLE_TUNNEL" == true ]]; then
  echo "      Starting Cloudflare Quick Tunnel for http://127.0.0.1:5000..."
  TUNNEL_LOG="$(mktemp /tmp/agentaudit-tunnel.XXXXXX.log)"
  cloudflared tunnel --url http://127.0.0.1:5000 >"${TUNNEL_LOG}" 2>&1 &
  TUNNEL_PID=$!

  # Wait up to 15 seconds for public trycloudflare.com URL to appear in logs
  for _ in {1..15}; do
    if ! kill -0 "$TUNNEL_PID" 2>/dev/null; then
      echo "WARNING: cloudflared tunnel process exited unexpectedly. Check tunnel log:"
      cat "${TUNNEL_LOG}" 2>/dev/null || true
      break
    fi

    FOUND_URL=$(grep -oE 'https://[a-zA-Z0-9-]+\.trycloudflare\.com' "${TUNNEL_LOG}" | head -n1 || true)
    if [[ -n "$FOUND_URL" ]]; then
      TUNNEL_URL="$FOUND_URL"
      break
    fi
    sleep 1
  done
fi

# 5. Start Frontend
echo "[4/4] Launching React/Vite frontend on port 5173..."
npm --prefix "${ROOT_DIR}/frontend" run dev &
FRONTEND_PID=$!

# Print Summary Banner
echo ""
echo "======================================================================"
echo "          AgentAudit Development Environment is Running!             "
echo "======================================================================"
echo "  Frontend Web UI   : http://127.0.0.1:5173"
echo "  Backend REST API  : http://127.0.0.1:5000"
echo "  Health Endpoint   : http://127.0.0.1:5000/health"
echo "  Local Webhook URL : http://127.0.0.1:5000/api/webhooks/paypal"

if [[ -n "$TUNNEL_URL" ]]; then
  echo "----------------------------------------------------------------------"
  echo "  Cloudflare Tunnel : ${TUNNEL_URL}"
  echo "  Public Webhook URL: ${TUNNEL_URL}/api/webhooks/paypal"
  echo ""
  echo "  Important Webhook Notice:"
  echo "  - Quick Tunnel URLs are ephemeral and regenerate on each script start."
  echo "  - Update your PayPal Sandbox App Webhook URL to match the public URL."
  echo "  - Ensure PAYPAL_WEBHOOK_ID in .env matches your registered webhook ID."
fi

echo "======================================================================"
echo "  Press [Ctrl+C] at any time to gracefully stop all processes.        "
echo "======================================================================"
echo ""

# Wait on background processes
wait -n "$BACKEND_PID" "$FRONTEND_PID"
