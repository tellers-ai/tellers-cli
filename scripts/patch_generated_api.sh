#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"/.. && pwd)"
MODELS_RS="${ROOT_DIR}/generated/tellers_api_client/src/models/mod.rs"

if [ ! -f "${MODELS_RS}" ]; then
  echo "Generated models module not found: ${MODELS_RS}" >&2
  exit 1
fi

# OpenAPI Generator 7.17 emits this unresolved inline-schema name for the
# backend's unconstrained JSON task result. Keep the source schema unchanged
# and provide the intended JSON value type in the generated Rust crate.
if ! grep -q '^pub type AnyOfLessThanGreaterThan = serde_json::Value;' "${MODELS_RS}"; then
  printf '\npub type AnyOfLessThanGreaterThan = serde_json::Value;\n' >> "${MODELS_RS}"
fi
