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

### Chat Commands

- `tellers "prompt"` — displays a minimal chat TUI from a streamed response
- `tellers --full-auto --background "prompt"` — starts a chat and prints only the chat id

### Upload Command

Upload media files to Tellers:

```bash
tellers upload /path/to/media_folder
```

**Common flags:**

- `--disable-description-generation` — Disable automatic time-based media description generation (enabled by default)
- `--dry-run` — Analyze files without uploading
- `--force-upload` — Upload files even if they were already uploaded
- `--local-encoding` — Enable local encoding before upload
- `--parallel-uploads <N>` — Number of parallel uploads (default: 4)
- `--ext <EXT>` — Filter files by extension (e.g., `--ext mp4 --ext mov`)
- `--in-app-path <PATH>` — Set the in-app path for uploaded files

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