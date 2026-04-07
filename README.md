# tellers-cli

Tellers CLI to interact with `tellers.ai` from the terminal.

## Installation

### Homebrew (macOS and Linux)

```bash
brew tap tellers-ai/tellers
brew install tellers
```

### Build from source

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

### Chat / Prompt

Run a prompt against the Tellers agent:

- **`tellers "prompt"`** — Streams the response and starts a minimal REPL (reply in the terminal; Ctrl-C to exit).
- **`tellers --background "prompt"`** — Single request, no REPL; prints the response text (or last JSON result when using `--json-response`).
- **`tellers --full-auto --background "prompt"`** — Same as `--background` with full-auto behavior.

**Prompt options:**

| Flag | Description |
|------|-------------|
| `--no-interaction` | Single response only, no REPL. |
| `--json-response` | Use the JSON endpoint; output is the last `tellers.json_result` event (no interaction implied). |
| `--tool <TOOL_ID>` | Enable a tool (repeat for multiple). Omit to use default tools from settings. |
| `--llm-model <MODEL>` | LLM model (e.g. `gpt-5.4-2026-03-05`). |
| `--interactive`, `-i` | Interactively set options: JSON response (y/N), no interaction (y/N), tool selection (checkbox list), and LLM model (list). |

**Examples:**

```bash
# Streamed chat with REPL
tellers "Generate a video, with cats"

# Single response, no follow-up
tellers --no-interaction "Generate a video, with stock footage video of cats"

# JSON endpoint: only the last JSON result is printed
tellers --json-response "Generate a video, with stock footage video of cats"

# Choose model and tools via flags
tellers --llm-model gpt-5.4-2026-03-05 --tool tool_a --tool tool_b "Your prompt"

# Interactive: prompts for JSON, no-interaction, checkbox list for tools, model picker
tellers -i "Generate a video, with stock footage video of cats"
```

**Interactive tool selection:** When you use `-i` and the settings include available tools, a TUI checkbox list is shown. Use **↑/↓** to move, **Space** to toggle, **a** to toggle all, **Enter** to confirm. Checkboxes are pre-set from each tool’s `enabled` field in the settings JSON (missing = enabled).

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

Files ≥ 10 MiB use **multipart S3 upload** (presigned part URLs, then complete); smaller files use a single presigned PUT.

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