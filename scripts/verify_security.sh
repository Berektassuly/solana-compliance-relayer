#!/usr/bin/env bash
# test_api.sh — Verify Security Protocol v2 (Nonces & Idempotency) for POST /transfer-requests
# Requires: curl, jq, node (with crypto for Ed25519). Uses inline Node for keypair + signing.

set -euo pipefail

# -----------------------------------------------------------------------------
# Configuration
# -----------------------------------------------------------------------------
API_URL="${API_URL:-http://localhost:3000}"
API_URL="${API_URL%/}"
# Single slash between base and path so we never request //transfer-requests
TRANSFER_REQUESTS_URL="${API_URL}/transfer-requests"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC}: $*"; }
fail() { echo -e "${RED}FAIL${NC}: $*"; exit 1; }
info() { echo -e "${YELLOW}INFO${NC}: $*"; }

TEMP_DIR=""
cleanup() { [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ] && rm -rf "$TEMP_DIR"; }
trap cleanup EXIT

ensure_node_deps() {
  TEMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t "test_api")
  SAVED_PWD=$(pwd)
  cd "$TEMP_DIR" || exit 1
  npm init -y >/dev/null 2>&1
  npm install tweetnacl bs58 >/dev/null 2>&1
  cd "$SAVED_PWD" || true
}

# Generate keypair: outputs PUB_B58 and SECRET_B58 (two lines)
run_keypair() {
  SAVED_PWD=$(pwd)
  cd "$TEMP_DIR" || exit 1
  node -e "
const nacl = require('tweetnacl');
const m = require('bs58');
const encode = m.encode || (m.default && m.default.encode);
const decode = m.decode || (m.default && m.default.decode);
const kp = nacl.sign.keyPair();
const pubB58 = encode(kp.publicKey);
const secB58 = encode(kp.secretKey);
console.log(pubB58);
console.log(secB58);
"
  cd "$SAVED_PWD" || true
}

# Sign message from:to:amount:mint:nonce; output signature base58 (message passed via file to avoid shell escaping)
run_sign() {
  local pubB58="$1" secB58="$2" from="$3" to="$4" amount="$5" mint="$6" nonce="$7"
  local msg="${from}:${to}:${amount}:${mint}:${nonce}"
  local msg_file="$TEMP_DIR/msg.txt"
  printf '%s' "$msg" > "$msg_file"
  SAVED_PWD=$(pwd)
  cd "$TEMP_DIR" || exit 1
  node -e "
const nacl = require('tweetnacl');
const m = require('bs58');
const encode = m.encode || (m.default && m.default.encode);
const decode = m.decode || (m.default && m.default.decode);
const fs = require('fs');
const message = fs.readFileSync('msg.txt', 'utf8');
const secretKey = decode(process.argv[1]);
const sig = nacl.sign.detached(Buffer.from(message, 'utf8'), secretKey);
console.log(encode(sig));
" "$secB58"
  cd "$SAVED_PWD" || true
}

# Generate nonce (UUID v4 or v7). Prefer uuidgen if available, else Node.
generate_nonce() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen
  else
    node -e "const u=require('crypto').randomUUID(); console.log(u);"
  fi
}

# -----------------------------------------------------------------------------
# Tests
# -----------------------------------------------------------------------------
main() {
  echo "=============================================="
  echo "  Security Protocol v2 — POST /transfer-requests"
  echo "  API: $API_URL"
  echo "=============================================="

  command -v curl >/dev/null 2>&1 || fail "curl is required"
  command -v jq   >/dev/null 2>&1 || fail "jq is required"
  command -v node >/dev/null 2>&1 || fail "node is required"

  info "Installing Node deps (tweetnacl, bs58) in a temp dir..."
  ensure_node_deps

  info "Generating keypair..."
  KEYPAIR_LINES=$(run_keypair)
  FROM_ADDRESS=$(echo "$KEYPAIR_LINES" | sed -n '1p')
  SECRET_B58=$(echo "$KEYPAIR_LINES" | sed -n '2p')
  TO_ADDRESS="DRpbCBMxVnDK7maPM5tGv6MvB3v1sRMC86PZ8okm21hy"
  AMOUNT="1000000000"
  MINT="SOL"

  info "Generating nonce..."
  NONCE=$(generate_nonce)

  info "Signing message {from}:{to}:{amount}:{mint}:{nonce}..."
  SIGNATURE=$(run_sign "$FROM_ADDRESS" "$SECRET_B58" "$FROM_ADDRESS" "$TO_ADDRESS" "$AMOUNT" "$MINT" "$NONCE")

  PAYLOAD=$(jq -n \
    --arg from "$FROM_ADDRESS" \
    --arg to   "$TO_ADDRESS" \
    --argjson amount $AMOUNT \
    --arg sig  "$SIGNATURE" \
    --arg nonce "$NONCE" \
    '{ from_address: $from, to_address: $to, transfer_details: { type: "public", amount: $amount }, token_mint: null, signature: $sig, nonce: $nonce }')

  # ----- Test 1: Happy path -----
  echo ""
  echo "--- Test 1: Happy path (new request) ---"
  RESP1=$(curl -s -w "\n%{http_code}" -X POST "$TRANSFER_REQUESTS_URL" \
    -H "Content-Type: application/json" \
    -H "Idempotency-Key: $NONCE" \
    -d "$PAYLOAD")
  HTTP1=$(echo "$RESP1" | tail -n1)
  BODY1=$(echo "$RESP1" | sed '$d')
  if [ "$HTTP1" != "200" ]; then
    fail "Test 1: Expected HTTP 200, got $HTTP1. Body: $BODY1"
  fi
  ID1=$(echo "$BODY1" | jq -r '.id')
  if [ -z "$ID1" ] || [ "$ID1" = "null" ]; then
    fail "Test 1: Response JSON missing .id. Body: $BODY1"
  fi
  pass "Test 1: HTTP 200 and JSON contains id ($ID1)"

  # ----- Test 2: Idempotency (replay) -----
  echo ""
  echo "--- Test 2: Idempotency (replay same request) ---"
  RESP2=$(curl -s -w "\n%{http_code}" -X POST "$TRANSFER_REQUESTS_URL" \
    -H "Content-Type: application/json" \
    -H "Idempotency-Key: $NONCE" \
    -d "$PAYLOAD")
  HTTP2=$(echo "$RESP2" | tail -n1)
  BODY2=$(echo "$RESP2" | sed '$d')
  if [ "$HTTP2" != "200" ]; then
    fail "Test 2: Expected HTTP 200 on replay, got $HTTP2. Body: $BODY2"
  fi
  ID2=$(echo "$BODY2" | jq -r '.id')
  if [ "$ID2" != "$ID1" ]; then
    fail "Test 2: Replay id ($ID2) does not match first request id ($ID1)"
  fi
  pass "Test 2: HTTP 200 on replay and same id ($ID2)"

  # ----- Test 3: Security (tampering: new nonce, old signature) -----
  echo ""
  echo "--- Test 3: Security check (tampering) ---"
  NONCE_TAMPER=$(generate_nonce)
  PAYLOAD_TAMPER=$(jq -n \
    --arg from "$FROM_ADDRESS" \
    --arg to   "$TO_ADDRESS" \
    --argjson amount $AMOUNT \
    --arg sig  "$SIGNATURE" \
    --arg nonce "$NONCE_TAMPER" \
    '{ from_address: $from, to_address: $to, transfer_details: { type: "public", amount: $amount }, token_mint: null, signature: $sig, nonce: $nonce }')
  RESP3=$(curl -s -w "\n%{http_code}" -X POST "$TRANSFER_REQUESTS_URL" \
    -H "Content-Type: application/json" \
    -H "Idempotency-Key: $NONCE_TAMPER" \
    -d "$PAYLOAD_TAMPER")
  HTTP3=$(echo "$RESP3" | tail -n1)
  BODY3=$(echo "$RESP3" | sed '$d')
  case "$HTTP3" in
    400|401|403) pass "Test 3: Signature verification rejected (HTTP $HTTP3)" ;;
    *) fail "Test 3: Expected HTTP 400/401/403, got $HTTP3. Body: $BODY3" ;;
  esac

  echo ""
  echo "=============================================="
  echo -e "  ${GREEN}All tests passed.${NC}"
  echo "=============================================="
}

main "$@"
