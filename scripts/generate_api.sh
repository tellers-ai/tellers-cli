#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"/.. && pwd)"
SPEC_PATH="${ROOT_DIR}/src/tellers_api/openapi.tellers_public_api.yaml"
OUT_DIR="${ROOT_DIR}/generated/tellers_api_client"

GENERATOR=""
if command -v openapi-generator >/dev/null 2>&1; then
  GENERATOR="openapi-generator"
elif command -v openapi-generator-cli >/dev/null 2>&1; then
  GENERATOR="openapi-generator-cli"
elif [ -n "${OPENAPI_GENERATOR_CLI_JAR:-}" ] && [ -f "${OPENAPI_GENERATOR_CLI_JAR}" ]; then
  GENERATOR="java -jar ${OPENAPI_GENERATOR_CLI_JAR}"
elif [ -f "${ROOT_DIR}/openapi-generator-cli.jar" ]; then
  GENERATOR="java -jar ${ROOT_DIR}/openapi-generator-cli.jar"
else
  echo "OpenAPI Generator not found. We use openapi-generator-cli 7.17.0. Install with version control:"
  echo "  - brew:  brew install openapi-generator@7.17"
  echo "  - npm:   npm i -g @openapitools/openapi-generator-cli"
  exit 1
fi

mkdir -p "${OUT_DIR}"

${GENERATOR} generate \
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

