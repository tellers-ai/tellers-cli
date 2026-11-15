# tellers-cli

Tellers CLI to interact with `tellers.ai` from the terminal.

## Installation

### Homebrew (macOS and Linux)

```bash
brew install tellers-ai/tellers/tellers
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

Or sign in with your Tellers account. This opens `auth.tellers.ai` in your
browser, completes a PKCE login through a local callback, and saves a
refreshable session for subsequent commands:

```bash
tellers login
tellers logout
```

The session is stored in the platform configuration directory with owner-only
permissions. Access tokens are refreshed automatically before commands run;
the API-key environment variable remains supported for backwards compatibility.

Optional:

```bash
export TELLERS_API_BASE=https://api.tellers.ai
```

## Staying up to date

`tellers` checks GitHub for a newer release at most once a day and prints an
upgrade hint on stderr (`brew upgrade tellers` for Homebrew installs). The check
has a 2-second timeout and never fails a command. It is skipped in CI, when stderr
is not a terminal, or when `TELLERS_NO_UPDATE_CHECK=1` is set.

Check right away:

```bash
tellers update
```

## Usage

### Chat / Prompt

Run a prompt against the Tellers agent:

- **`tellers "prompt"`** — Streams the response and starts a minimal REPL (reply in the terminal; Ctrl-C to exit).
- **`tellers --background "prompt"`** — Single request, no REPL; prints the agent's answer as text (or the JSON result when using `--json-response`).
- **`tellers --full-auto --background "prompt"`** — Same as `--background` with full-auto behavior.

**Prompt options:**

| Flag | Description |
|------|-------------|
| `--no-interaction` | Single response only, no REPL. With `--export-to`, also sends without asking to confirm the full prompt (needed when stdin is not a terminal). |
| `--json-response` | Output the turn's result as JSON (`status`, `chat_id`, `projects`, `assets`, `message`), from `GET /agent/chat/{id}/result` (no interaction implied). |
| `--export-to <DIR>` | Make the agent finish the job on its own, export each project it built to MP4, and download each to `DIR/<project name>.mp4` (no interaction implied). See below. |
| `--export-rendition <RES>` | Rendition(s) for `--export-to` (`360p`…`4k`, repeat or comma-separate). Default `1080p`. |
| `--export-force` | Replace existing files in the `--export-to` directory (otherwise `<name> (2).mp4`, …). |
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

# Prompt → project → MP4 on disk, in one command
tellers --export-to ./renders "Search a stock video of ocean waves at sunset and add the title 'Golden Hour'"

# In 720p, without the REPL's streaming (prints the answer and saved paths)
tellers --background --export-to ./renders --export-rendition 720p "Generate a video of cats"

# Choose model and tools via flags
tellers --llm-model gpt-5.4-2026-03-05 --tool tool_a --tool tool_b "Your prompt"

# Interactive: prompts for JSON, no-interaction, checkbox list for tools, model picker
tellers -i "Generate a video, with stock footage video of cats"
```

**How `--export-to` works:** sign in once with `tellers login`. Before anything is sent, the CLI shows the full prompt (yours plus the instructions below) and asks `Is this prompt valid? Send it? [y/n]`. On yes, it sends the prompt and waits; on no, nothing is sent. The CLI appends instructions to your prompt telling the agent to do the request without asking questions, export each project it built with the export tool (waiting for the render), and end its answer with one line per project:

```
TELLERS_EXPORT {"project_id": "…", "project_name": "Golden Hour", "asset_id": "…", "completed": true}
```

The turn streams as usual. Once it ends, the CLI reads those lines and downloads each export asset to `DIR/<project name>.mp4`. If the agent reports an export as not completed (or gives no asset), the CLI exports the project itself and downloads the result. With no `TELLERS_EXPORT` line, the command fails.

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

Files ≥ 100 MiB use a **multipart S3 upload**: the file is read and sent 50 MiB at a time, each part to a URL fetched just before it is sent (and fetched again for a retry), then the upload is completed. Smaller files use a single presigned PUT.

### Asset Download Command

Download a video asset using its asset ID:

```bash
tellers asset download <asset-id> --output /path/to/video.mp4
```

Use `--quality highest|lowest|original|480p|720p|1080p` to select a rendition. Existing files are protected by default; pass `--force` to replace one.

### Project Export Command

Render a project to MP4, wait for it, and download it:

```bash
tellers project export <project-id> --output video.mp4
```

`--download` (`-d`) saves to `<project-id>.mp4`; `--output` (`-o`) picks the path. Use `-r 720p,1080p` for renditions (default `1080p`) and `--force` to replace an existing file. Without `--download`/`--output`, the command only starts the export and prints `task_id` / `asset_id` (add `--wait` to wait for it).

### Project OTIO Export Command

Export a project timeline as OpenTimelineIO:

```bash
# Timeline JSON; clips point at presigned media URLs
tellers project export-otio <project-id> --output timeline.otio

# Self-contained bundle: timeline plus every referenced media file
tellers project export-otio <project-id> --output timeline.otioz
```

- `--format otio|otioz` — Output format (inferred from the `--output` extension; defaults to `otio`)
- `--rendition highest|lowest|original|480p|720p|1080p` — Media rendition referenced or bundled (default: `highest`)
- `--presign-expires-in <SECONDS>` — Lifetime of presigned media URLs, 3600–604800 (default: 43200)
- `--redirect-urls` — Use authenticated `/asset/url/{asset_id}` redirect links instead of presigned URLs (`otio` only)
- `--force` — Replace an existing destination file

An `.otioz` bundle follows the OpenTimelineIO layout (`version.txt`, `content.otio`, `media/`) with clip `target_url`s rewritten to `media/<asset-id>.<ext>`.

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
