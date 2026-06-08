#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# OCI Distribution Spec v1.1 conformance harness for ferro-oci-server.
#
# This script:
#   1. builds the `ferro-oci-server` binary;
#   2. boots it on an ephemeral port with a temp FS-backed blob store;
#   3. obtains the official `opencontainers/distribution-spec`
#      conformance test binary (via a local Go toolchain *or* the
#      prebuilt conformance container image);
#   4. points `OCI_ROOT_URL` at the running server and enables all four
#      workflow categories (push, pull, content-discovery,
#      content-management);
#   5. runs the suite and copies the JUnit/HTML report next to this
#      script;
#   6. tears the server down.
#
# It is intentionally side-effect-local: everything lands under a temp
# dir except the final report, which is copied to
# `tests/conformance/report/`.
#
# Usage:
#   tests/conformance/run_conformance.sh            # auto-detect runner
#   CONFORMANCE_RUNNER=go     tests/conformance/run_conformance.sh
#   CONFORMANCE_RUNNER=docker tests/conformance/run_conformance.sh
#
# Environment:
#   FERRO_OCI_PORT        port to bind the server (default: 15000)
#   CONFORMANCE_REF       distribution-spec git ref (default: v1.1.0)
#   CONFORMANCE_IMAGE     prebuilt conformance image
#                         (default: ghcr.io/opencontainers/distribution-spec/conformance:v1.1.0)
#   CONFORMANCE_RUNNER    go | docker | auto (default: auto)
#
# Exit code mirrors the conformance suite's own exit code, so CI can
# gate on it.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
REPO_ROOT="$(cd "${CRATE_DIR}/../.." && pwd)"
REPORT_DIR="${SCRIPT_DIR}/report"

PORT="${FERRO_OCI_PORT:-15000}"
ROOT_URL="http://127.0.0.1:${PORT}"
CONFORMANCE_REF="${CONFORMANCE_REF:-v1.1.0}"
CONFORMANCE_IMAGE="${CONFORMANCE_IMAGE:-ghcr.io/opencontainers/distribution-spec/conformance:v1.1.0}"
CONFORMANCE_RUNNER="${CONFORMANCE_RUNNER:-auto}"

WORKDIR="$(mktemp -d)"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill -TERM "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

log() { printf '[conformance] %s\n' "$*" >&2; }

# --- 1 + 2. Build and boot the server ---------------------------------------
log "building ferro-oci-server binary"
cargo build --quiet -p ferro-oci-server --bin ferro-oci-server

BIN="${REPO_ROOT}/target/debug/ferro-oci-server"
[[ -x "${BIN}" ]] || { log "binary not found at ${BIN}"; exit 70; }

log "starting server on ${ROOT_URL} (storage: ${WORKDIR}/blobs)"
FERRO_OCI_LISTEN="127.0.0.1:${PORT}" \
FERRO_OCI_STORAGE_DIR="${WORKDIR}/blobs" \
  "${BIN}" >"${WORKDIR}/server.log" 2>&1 &
SERVER_PID=$!

# Wait for /v2/ to answer.
for _ in $(seq 1 50); do
  if curl -fsS -o /dev/null "${ROOT_URL}/v2/" 2>/dev/null; then
    break
  fi
  sleep 0.2
done
curl -fsS -o /dev/null "${ROOT_URL}/v2/" || { log "server never came up; log:"; cat "${WORKDIR}/server.log" >&2; exit 71; }
log "server is up"

# --- 3. Pick a runner -------------------------------------------------------
runner="${CONFORMANCE_RUNNER}"
if [[ "${runner}" == "auto" ]]; then
  if command -v go >/dev/null 2>&1; then
    runner="go"
  elif command -v docker >/dev/null 2>&1; then
    runner="docker"
  else
    log "ERROR: neither 'go' nor 'docker' is available — cannot run the official suite."
    log "Install the Go toolchain (>=1.21) or Docker, then re-run."
    exit 69   # EX_UNAVAILABLE
  fi
fi
log "using runner: ${runner}"

mkdir -p "${REPORT_DIR}"

# All four conformance workflow categories.
export OCI_ROOT_URL="${ROOT_URL}"
export OCI_NAMESPACE="ferro/conformance"
export OCI_TEST_PULL=1
export OCI_TEST_PUSH=1
export OCI_TEST_CONTENT_DISCOVERY=1
export OCI_TEST_CONTENT_MANAGEMENT=1
export OCI_HIDE_SKIPPED_WORKFLOWS=0
export OCI_DEBUG=1
export OCI_REPORT_DIR="${REPORT_DIR}"

run_rc=0
case "${runner}" in
  go)
    SUITE_DIR="${WORKDIR}/distribution-spec"
    log "cloning distribution-spec@${CONFORMANCE_REF}"
    git clone --depth 1 --branch "${CONFORMANCE_REF}" \
      https://github.com/opencontainers/distribution-spec "${SUITE_DIR}"
    log "building conformance.test"
    ( cd "${SUITE_DIR}/conformance" && go test -c -o "${WORKDIR}/conformance.test" )
    log "running conformance.test against ${OCI_ROOT_URL}"
    ( cd "${REPORT_DIR}" && "${WORKDIR}/conformance.test" ) || run_rc=$?
    ;;
  docker)
    log "running prebuilt conformance image ${CONFORMANCE_IMAGE}"
    docker run --rm --network host \
      -e OCI_ROOT_URL -e OCI_NAMESPACE \
      -e OCI_TEST_PULL -e OCI_TEST_PUSH \
      -e OCI_TEST_CONTENT_DISCOVERY -e OCI_TEST_CONTENT_MANAGEMENT \
      -e OCI_HIDE_SKIPPED_WORKFLOWS -e OCI_DEBUG \
      -v "${REPORT_DIR}:/report" -e OCI_REPORT_DIR=/report \
      "${CONFORMANCE_IMAGE}" || run_rc=$?
    ;;
  *)
    log "unknown CONFORMANCE_RUNNER='${runner}' (want: go|docker|auto)"
    exit 64
    ;;
esac

log "conformance suite exit code: ${run_rc}"
log "report written to: ${REPORT_DIR}"
if [[ -f "${REPORT_DIR}/junit.xml" ]]; then
  log "JUnit summary:"
  grep -o 'testsuites[^>]*' "${REPORT_DIR}/junit.xml" | head -1 >&2 || true
fi
exit "${run_rc}"
