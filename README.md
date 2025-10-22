# tellers-cli

Tellers CLI to interact with `tellers.ai` from the terminal.

## Quickstart

Build the CLI:

```bash
# Generate the client crate
scripts/generate_api.sh

# Build the CLI
cargo build --release
```

Set your API key:

```bash
export TELLERS_API_KEY=sk_...
```

Optional:

```bash
export TELLERS_API_BASE=https://api.tellers.ai
```

## Usage

- `tellers "prompt"` — displays a minimal chat TUI from a streamed response
- `tellers --full-auto --background "prompt"` — starts a chat and prints only the chat id
- `tellers upload --only-proxies /path/to/media_folder` — uploads media folder (proxies only if flag set)

## Implementation Notes

- Argument parsing via Clap 4.x. See `clap` docs: [docs.rs/clap](https://docs.rs/clap/latest/clap/)
- Minimal TUI via Ratatui. See `ratatui` site: [ratatui.rs](https://ratatui.rs/)
- API client generated from OpenAPI spec using `openapi-generator`. See `scripts/generate_api.sh`.

## Generate API client from OpenAPI

Requires `openapi-generator`:

```bash
brew install openapi-generator
```

Spec location (update as needed): `src/tellers_api/openapi.tellers_public_api.yaml`

Generate the client crate:

```bash
scripts/generate_api.sh
```