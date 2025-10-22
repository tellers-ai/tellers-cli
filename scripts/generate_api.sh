#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"/.. && pwd)"
SPEC_PATH="${ROOT_DIR}/src/tellers_api/openapi.tellers_public_api.yaml"
OUT_DIR="${ROOT_DIR}/generated/tellers_api_client"

if ! command -v openapi-generator >/dev/null 2>&1; then
  echo "openapi-generator not found. Install via:"
  echo "  brew install openapi-generator"
  exit 1
fi

mkdir -p "${OUT_DIR}"

openapi-generator generate \
  -i "${SPEC_PATH}" \
  -g rust \
  -o "${OUT_DIR}" \
  --additional-properties=packageName=tellers_api_client,packageVersion=0.1.0,library=reqwest,supportAsync=true,reqwestClient=true

echo "Generated client at ${OUT_DIR}"
 
# Inject crate-wide lint allowances for generator naming quirks
LIB_RS="${OUT_DIR}/src/lib.rs"
if [ -f "${LIB_RS}" ]; then
  TMP_LIB="${LIB_RS}.tmp"
  {
    echo "#![allow(non_snake_case)]";
    echo "#![allow(dead_code)]";
    cat "${LIB_RS}";
  } > "${TMP_LIB}"
  mv "${TMP_LIB}" "${LIB_RS}"
  echo "Patched ${LIB_RS} with lint allowances."
fi



