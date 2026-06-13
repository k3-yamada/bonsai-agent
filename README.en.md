# bonsai-agent

**English** | [日本語](README.md)

An autonomous AI agent written in Rust, powered by Bonsai-8B (a 1-bit quantized Qwen3-8B, 1.28 GB).

Runs entirely on a Mac M2 (16 GB). No external cloud API required. It executes tasks autonomously and learns from experience using a local LLM alone.

## Features

- **1.28 GB LLM** — Bonsai-8B (1-bit quantized) handles tool calls, code understanding, and web search
- **Self-evolution** — automatically records experience, promotes a tool chain to a skill after 3 successes, and ingests arXiv papers to grow its knowledge
- **Flow → Stock** — automatically extracts decisions, learnings, and TODOs from conversations into Markdown files (the "Karpathy pattern")
- **Safety by design** — sandbox, path guard, secret filter, graduated autonomy levels, safe mode
- **Extensible** — TOML plugins, MCP client, pre/post hooks
- **Rich harness patterns** — raises the reliability of a 1-bit model under the "Scaffolding > Model" principle (~1,500 tests; design principles in [CLAUDE.md](CLAUDE.md) / [docs/quality/lab-history.md](docs/quality/lab-history.md))
- **Design-philosophy anchor ([VALUES.md](docs/VALUES.md))** — values V1–V7 written down, with Goodhart's-Law monitoring (env-gated metric-degeneration detection) to guard against self-drift
- **LLM-as-judge evaluation harness** — Judge Gate + rubric scoring, expanding the benchmark from 22 → 40 tasks
- **MLX backend support** — in addition to llama-server, inference can run on mlx-lm (Apple Silicon optimized)
- **Memory-optimized sidecar** — `start-mlx-sidecar.sh` enables KV-cache quantization (-71%) + `mx.set_cache_limit` control to prevent swap on a 16 GB M2
- **Middleware chain** — a 5-stage pipeline informed by DeerFlow (Audit → ToolTrack → Stall → Compact → TokenBudget)
- **Parallel read tools** — two or more consecutive reads are auto-parallelized (writes act as a sequential barrier)
- **Type-driven tool definitions** — schemars `JsonSchema` derive auto-generates schemas + type-safe parsing (the `TypedTool` trait)
- **TTL freshness management** — an `expires_at` column + auto-purge at session start prevents stale information
- **Automatic ADR generation** — decisions made during Replan/Advisor interventions are accumulated as Markdown in the Vault
- **Invariant checks** — tool success rate and answer quality are automatically verified on task completion

## Quick start

### 1. Set up Bonsai-demo (first time only)

```bash
cd ~
git clone https://github.com/PrismML-Eng/Bonsai-demo.git
cd Bonsai-demo
sh scripts/download_binaries.sh
curl -L -o models/gguf/8B/Bonsai-8B.gguf \
  "https://huggingface.co/prism-ml/Bonsai-8B-gguf/resolve/main/Bonsai-8B.gguf"
```

### 2. Start llama-server

```bash
cd ~/bonsai-agent
./scripts/start-server.sh
```

### 3. Start bonsai-agent

```bash
cargo run
```

```
bonsai> List the files in this directory
bonsai> Read the contents of Cargo.toml
bonsai> Search the web about Rust
bonsai> exit
```

### MLX backend (alternative for Apple Silicon)

The MLX build of Ternary Bonsai 8B can be started in two ways:

```bash
# Setup (first time only)
./scripts/setup_mlx_ternary.sh

# A. cubist mlx-openai-server (port 8000, standard)
./scripts/start-mlx-server.sh &

# B. memory-optimized sidecar (port 8888, recommended for M2 16GB)
./scripts/start-mlx-sidecar.sh &
```

**Option B (sidecar)** implements KV-cache quantization (-71%, kv4@6417tok) and swap prevention via `mx.set_cache_limit`. It is controlled by env vars such as `BONSAI_MLX_CACHE_LIMIT_GB` / `BONSAI_MLX_KV_BITS` / `BONSAI_MLX_QUANTIZED_KV_START`. See [docs/execution/runbook.md](docs/execution/runbook.md) §Phase 2 memory optimization for details.

Switch the backend in `config.toml` (match the port to the script you use):

```toml
[model]
backend = "mlx-lm"
server_url = "http://localhost:8888"  # when using the sidecar; cubist is 8000
model_id = "ternary-bonsai-8b"
context_length = 65536
```

### Mock mode (run without an LLM)

```bash
cargo run -- --mock --exec "hello"
cargo run -- --mock    # interactive mode
```

## CLI

```
cargo run                              # interactive mode
cargo run -- --exec "..."              # one-shot execution
cargo run -- --mock                    # mock mode (no LLM)
cargo run -- --sessions                # list sessions
cargo run -- --resume <ID>             # resume a session
cargo run -- --tasks                   # list incomplete tasks
cargo run -- --audit                   # audit log
cargo run -- --vault                   # knowledge Vault overview
cargo run -- --manifest                # capability list
cargo run -- --list-tools              # registered tools (live registry after whitelist)
cargo run -- --ingest <PATH>           # ingest .md/.txt into memory (knowledge daemon)
cargo run -- --dashboard               # unified stats dashboard
cargo run -- --checkpoints             # list checkpoints
cargo run -- --rollback <id>           # restore a checkpoint
cargo run -- --lab                     # autonomous self-improvement loop
cargo run -- --init                    # generate a config.toml template
cargo run -- --skills-export           # export skills to Markdown
cargo run -- --diagnose                # diagnose server connectivity
cargo run -- --evolve                  # arXiv ingestion + self-improvement
cargo run -- --serve --api-port <PORT> # REST API server
cargo run -- --mcp-server              # run as an MCP server
cargo run -- --server-url <URL>        # custom server URL
```

### Environment variables (key ones)

For the full list, see [docs/execution/runbook.md](docs/execution/runbook.md).

| Env var | Default | Purpose |
|---------|---------|---------|
| `BONSAI_DB_PATH` | OS data_dir | Override the memory DB path (used for test isolation / keeping production clean) |
| `BONSAI_ENABLED_TOOLS` | unset | Deny-by-default tool whitelist. Only the listed tools are enabled (all tools when unset) |
| `BONSAI_LAB_SMOKE` | unset | Smoke mode. Auto-allows read-only tools only + shrinks context |

```bash
# Example: safely sanity-check with only read-only tools enabled
BONSAI_ENABLED_TOOLS=file_read,recall cargo run -- --list-tools
# => 2 registered tools: file_read / recall

# Smoke mode (read-only default = file_read/repo_map/recall/web_fetch/web_search/arxiv_search)
BONSAI_LAB_SMOKE=1 cargo run -- --list-tools
```

## Build feature flags

`Cargo.toml` has `default = ["cli", "tree-sitter", "embeddings"]`, so **all three features are ON by default**. `cargo build` / `cargo run` enable the full setup (sqlite-vec vec0 KNN + fastembed + tree-sitter) with no extra flags.

| feature | contents | default |
|---|---|---|
| `cli` | clap CLI flags | ✅ ON |
| `tree-sitter` | RepoMap (Rust/Python/TS/JS/Go) | ✅ ON |
| `embeddings` | fastembed (AllMiniLML6V2) + sqlite-vec vec0 ANN search | ✅ ON |

For a **hash-only / lightweight build** (CI, tests, embedded use), opt out explicitly:

```bash
cargo build --release --no-default-features --features cli,tree-sitter
```

In this case `HybridSearch::vector_search` switches to a linear-scan path (compile-time exclusive, no runtime branch). Embeddings fall back to `SimpleEmbedder` (hash-based), so semantic search does not work, but the build/tests complete.

### Local embeddings (offline / via MLX)

The `embeddings` feature internally **downloads** the `ort` (ONNX Runtime) prebuilt binary **at build time** and **downloads** the `fastembed` model **from Hugging Face at runtime**. Both fail in a network-restricted environment.

To stay fully local, use the MLX sidecar's `/v1/embeddings` (OpenAI-compatible) endpoint:

```bash
# 1. Start the sidecar (mlx-embeddings bundled; the embedding model is lazy-loaded on first request)
./scripts/start-mlx-sidecar.sh &

# 2. Enable HttpEmbedder on the bonsai side
export BONSAI_EMBED_URL=http://localhost:8888
cargo run --no-default-features --features cli,tree-sitter
```

When `BONSAI_EMBED_URL` is set, `create_embedder()` prefers `HttpEmbedder` (independent of fastembed/ONNX), so **real embeddings work without downloading the ort binary**. If the sidecar is down, it gracefully falls back to hash embeddings. The embedding model is configurable via `BONSAI_MLX_EMBED_MODEL` (default `mlx-community/all-MiniLM-L6-v2-4bit`).

## Tools

| Tool | Permission | Function |
|------|------------|----------|
| `shell` | Confirm | Run shell commands (via sandbox) |
| `file_read` | Auto | Read files (parallel-capable) |
| `file_write` | Confirm | Write files (full content or search/replace diff, 9 fuzzy strategies) |
| `multi_edit` | Confirm | Batch-edit multiple spots in a single file (atomic) |
| `git` | Confirm | Git operations (status/diff/log/commit/add/branch) |
| `web_search` | Auto | Web search (DuckDuckGo API) |
| `web_fetch` | Auto | Fetch text from a URL |
| `repo_map` | Auto | Code structure map (Rust/Python/TS/JS/Go/Java/C/C++/Kotlin/Swift) |
| `arxiv_search` | Auto | Search arXiv papers |
| `remember` | Auto | Store facts/preferences into memory (knowledge daemon) |
| `recall` | Auto | Search past memory (knowledge daemon, FTS5 + vector) |
| **Plugins** | Configurable | Add custom tools via TOML |
| **MCP** | Confirm | Use tools from MCP servers |

You can restrict which tools are enabled with `BONSAI_ENABLED_TOOLS` / `BONSAI_LAB_SMOKE` (deny-by-default; see "Environment variables" above). Use `--list-tools` to inspect the actually-enabled tool set.

Read-only tools (`file_read`, `web_search`, `web_fetch`, `repo_map`) are auto-parallelized when two or more run consecutively, via the `is_read_only()` trait. Write tools act as a barrier guaranteeing sequential execution.

## Architecture

```
User input
 ↓
Hybrid search (FTS5 + vector) → inject relevant memory into the prompt
 ↓
Past experience (success/failure) → inject into the prompt
 ↓
LLM inference (Bonsai-8B via llama-server / mlx-lm)
 ↓
Parse → validate → execute tool
 ↓                              ↓
Apply secret filter           Record audit log
 ↓
Middleware chain (5-stage pipeline)
 ├── AuditMiddleware       — per-step audit
 ├── ToolTrackMiddleware   — tool-usage tracking
 ├── StallMiddleware       — stall detection → replan
 ├── CompactMiddleware     — context compaction
 └── TokenBudgetMiddleware — token-budget management
 ↓
Auto-record experience → promote to skill after 3 successes
 ↓
Knowledge Vault (Flow → Stock auto-extraction → accumulate Markdown files)
 ↓
Session persistence
```

## Configuration

`~/Library/Application Support/bonsai-agent/config.toml` (macOS) or `~/.config/bonsai-agent/config.toml` (Linux). Optional — it runs with defaults if absent.

Generate a template with `cargo run -- --init`.

```toml
[model]
server_url = "http://localhost:8080"
model_id = "bonsai-8b"
context_length = 16384
# backend = "mlx-lm"  # when using the MLX backend

[model.inference]
temperature = 0.5
top_p = 0.85
top_k = 20
min_p = 0.05
max_tokens = 1024
repeat_penalty = 1.15

[agent]
max_iterations = 10
max_retries = 3
shell_timeout_secs = 30
auto_checkpoint = true

[advisor]
# api_key is auto-detected from the OPENAI_API_KEY / ANTHROPIC_API_KEY env vars
# api_model = "gpt-4o-mini"
# timeout_secs = 30

[experiment]
dreamer_interval = 5
max_experiments = 10

[safety]
deny_paths = ["~/.ssh", "~/.gnupg", "~/.aws"]

[hooks]
pre_tool = ["echo $BONSAI_TOOL_NAME"]
post_tool = ["logger -t bonsai $BONSAI_TOOL_NAME"]

[[plugins.tools]]
name = "weather"
command = "curl -s 'wttr.in/{location}?format=3'"
description = "Get the weather"
permission = "auto"
[plugins.tools.parameters]
location = { type = "string", description = "City name" }

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]

# HTTP transport (Streamable HTTP MCP)
# [[mcp.servers]]
# name = "remote"
# command = ""
# args = []
# url = "http://localhost:3000/mcp"
```

## Knowledge Vault

Automatically extracts important information (Stock) from conversations (Flow) and accumulates it into Markdown files.

```
# macOS: ~/Library/Application Support/bonsai-agent/vault/
# Linux: ~/.local/share/bonsai-agent/vault/
vault/
├── decisions.md      # decisions ("decided to ...")
├── facts.md          # facts ("... is the case")
├── preferences.md    # preferences ("... is preferred")
├── patterns.md       # patterns
├── insights.md       # insights ("turns out that ...")
└── todos.md          # to-dos ("need to ...")
```

Directly viewable/editable in Obsidian, etc.

## Self-evolution

```
auto-record experience → accumulate memory → search/inject next time → promote to skill
                                                              ↑
auto-ingest arXiv papers → accumulate knowledge ─────────────┘
```

- Auto-records successes/failures/insights (`ExperienceStore`)
- The same tool chain succeeding 3 times → auto-promoted to a skill (`SkillStore`)
- Detects and logs user corrections ("no", "wrong") / reinforcement ("perfect", "I see") in the agent loop (JP/EN, `detect_feedback`; DB persistence and search-weight updates are future work)
- Auto-collects papers in areas of interest from the arXiv API (`EvolutionEngine`)
- Periodic reflection reports (`Dreamer`)
- **Proactive self-improvement** (`apply_improvements`):
  - Auto-detects command patterns that failed 3+ times → accumulates a warning memory
  - Detects missing skills → auto-records a suggestion to skill-ify
  - Detects declining success rate → auto-records a prompt-improvement suggestion
  - Detects frequently-used tools → auto-records a priority for accuracy improvement
  - All accumulated in memory and auto-injected into the prompt next session

## Lab benchmark results

Mutation evaluation on Bonsai-8B 1-bit, k=3, 10-cycle paired. See [docs/quality/lab-history.md](docs/quality/lab-history.md) for the full history and details.

### Status (as of 2026-06)
- **Ceiling: 10 consecutive REJECTs** (v17–v21) — the default settings have converged on the optimum; the harness machinery is already optimized
- **Paired-evidence discipline** ([ADR-003](docs/decisions/ADR-003-paired-evidence-over-unpaired.md)) — several unpaired single-cycle ACCEPTs were overturned on paired re-evaluation → decisively eliminating cherry-picked noise
- **Capability profile** (AgentFloor T1–T6): T1 Instruct=0.68 / T3 ToolSelect=0.77 / **T6 LongHorizon=0.47 (weakest)** — tier-targeted mutations attack the T6 bias
- baseline score ≈ 0.82 (smoke: score=0.8209 / pass@k=1.0)

### Defaulted mutations (Lab ACCEPT → permanently applied)
- "Plan enforcement" rule (+0.025, v1)
- "Describe intent in `<think>` before using a tool" (+0.032, v5)
- "Fallback strategy" (+0.001, v5)
- "Verify file contents before answering" (+0.0157, v9)

## Harness patterns

Reliability-improvement patterns for a 1-bit model, based on the "Scaffolding > Model" design principle (representative examples):

- **pass^k evaluation**: run each task k times, detect mutation effects by consecutive success rate
- **Continue Sites**: 3-stage escalation of consecutive failure → retry → replan → safe stop
- **2-layer LoopDetector**: salient hash + frequency threshold + cyclic-pattern detection
- **StallDetector**: no-progress detection → replan injection via Advisor
- **9 fuzzy-match strategies**: whitespace normalization / trim / flexible indent / Unicode / escape / block anchor / boundary trim
- **Deferred Schema**: tool-schema name + description only, saving 80% of tokens
- **Staged separation pipeline**: complex-task detection → auto-injects a planning pre-step
- **Event Sourcing**: a unified event stream (replay/analysis ready)
- **Advisor Tool**: simplification directives + pre-completion self-verification + HttpAdvisor (delegation to an OpenAI-compatible API)
- **Middleware chain**: `trait Middleware` + `MiddlewareChain` (5-stage pipeline)
- **Parallel read tools**: `is_read_only()` + `std::thread::scope`
- **MLX backend**: `ServerBackend` enum (llama-server/mlx-lm switch)
- **InferenceParams**: temperature/top_p/top_k/min_p/max_tokens/repeat_penalty configurable
- **12 structured error classes**: extended `FailureMode` + `RecoveryHint`
- **Unified health check**: /health + /v1/models fallback (MLX-aware)

For design principles and representative patterns, see [CLAUDE.md](CLAUDE.md). (The exhaustive experiment log is kept in internal developer notes.)

## Development

```bash
cargo test --lib               # ~1,500 tests
cargo test --test structural   # layer/size/eprintln lint (Z-4)
cargo clippy --lib -- -D warnings  # lint
cargo fmt -- --check           # formatting
```

For development-flow details (Lab startup, env list, smoke procedure) see [docs/execution/runbook.md](docs/execution/runbook.md); for design decisions see [docs/decisions/](docs/decisions/) (ADR-001–011); for design philosophy see [docs/VALUES.md](docs/VALUES.md).

## Requirements

- macOS (Apple Silicon) or Linux
- Rust 1.80+ (edition 2024)
- llama-server (obtain from [PrismML Bonsai-demo](https://github.com/PrismML-Eng/Bonsai-demo))
- or mlx-lm + mlx-openai-server (set up via `./scripts/setup_mlx_ternary.sh`)

## License

MIT
