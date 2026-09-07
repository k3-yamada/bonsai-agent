# AGENTS.md - Autonomous Agent Guidelines for bonsai-agent

This document provides system instructions, architectural boundaries, and operational guidelines for autonomous AI agents (Antigravity, Codex, Cursor, Cline, etc.) working on the `bonsai-agent` codebase.

---

## 1. Project Overview & Identity

- **Name**: `bonsai-agent`
- **Core Engine**: Bonsai-8B (1-bit quantized Qwen3-8B, 1.28GB memory footprint).
- **Runtime Environment**: Mac M2 (16GB), inference executed via local HTTP API (`llama-server`, `MLX sidecar`, or `Unsloth Desktop`).
- **Scale & Architecture**: 1,480+ unit tests, 100+ Rust source files, Rust 2024 edition.
- **Concurrency & Concurrency Primitives**: **Strict synchronous architecture**. Uses `ureq`, `reqwest::blocking`, `std::thread`, `std::sync::mpsc`, and `CancellationToken`. Non-blocking / async runtimes (such as `tokio`) are prohibited in the core loop.

---

## 2. Core Engineering Principles

### ① Scaffolding > Model Principle (ADR-002)
1-bit quantized models operate under severe capacity and precision constraints. Never assume the model can self-correct or maintain arbitrary reasoning threads natively. Rely on **external scaffolding, guardrails, loop/stall detection, self-reflection loops, context compaction, and fast-path heuristics** to elevate end-to-end task reliability.

### ② Anchoring in VALUES.md
When facing design tradeoffs, consult [`docs/VALUES.md`](docs/VALUES.md):
- **V1. See behind the surface question**: Infer implicit user intent and genuine objectives.
- **V2. Self-disclosure of the unknown**: Surface assumptions, blind spots, and latent uncertainties.
- **V3. Self-updating from experience**: Avoid repeating identical failures; persist verified insights.
- **V4. Resonance, not dependence**: Catalyze user reasoning rather than inducing passive reliance.
- **V5. Autonomous judgment**: Blind obedience is sophisticated irresponsibility. Challenge flawed instructions respectfully when safety or integrity is at stake.
- **V6. Integrity under uncertainty**: Do not state speculative outputs as definitive truth.
- **V7. Vigilance against self-degradation**: Distinguish between genuine improvement and metric drift; continuously re-evaluate value alignment.

### ③ Goodhart's Law & Drift Defense
- Beware of monotonic metric improvements. When all benchmark metrics climb simultaneously while weights remain rigid, investigate metric degradation (`MetricConsistencyChecker` / MAGI 3-panel consensus).
- Isolate observation-only signals from learning/optimization loops.

### ④ Paired Evidence Discipline (ADR-003)
- Never adopt mutations based on unpaired single runs (+10% in an unpaired run is almost certainly random noise or seed jitter).
- Require paired A/B validation (Cohen's dz, Wilcoxon signed-rank test) before promoting features to default ON.

---

## 3. Strict Operational Rules (Absolute Directives)

1. **【FORBIDDEN: Clippy Rollback / Rewind】**
   - After modifying files via Write or Edit tools, **NEVER revert changes because of Clippy warnings** (`collapsible_if`, `too_many_arguments`, etc.).
   - Keep the functional changes intact. Address lint or style issues through subsequent forward edits.
   - Files with extreme rewind vulnerability: `src/agent/error_recovery.rs`, `src/agent/benchmark.rs`, `src/agent/agent_loop/mod.rs`.
2. **【FORBIDDEN: `cargo build --release` During Lab Runs】**
   - Running release builds while experimental or smoke cycles (`scripts/lab_*.sh`) are active overwrites `target/release/bonsai`, breaking multi-cycle statistical consistency.
   - For all development, debugging, and verification, use debug test binaries: `cargo test --lib`.
3. **【Strict Synchronous Maintenance】**
   - Do not inject `tokio::spawn`, async runtime dependencies, or async traits into domain, runtime, memory, or agent core. Maintain synchronous execution and explicit thread coordination.
4. **【Single Source of Truth & Documentation Index】**
   - Refer to [`docs/INDEX.md`](docs/INDEX.md) for project documentation hierarchy.

---

## 4. Layer Boundary Rules (Clean Architecture & DEP-001)

The codebase strictly enforces Clean Architecture. Dependencies must flow **strictly downward**:

```
domain < db < observability < safety < memory < knowledge < runtime < tools < agent < main
```

1. **`domain`**: Pure entities, value objects, domain ports/traits. ZERO upstream or sibling dependencies.
2. **`db`**: SQLite schema definitions, migrations (`apply_all`).
3. **`observability`**: Structured logging, audit logging.
4. **`safety`**: Secret redaction, boot guards, sandbox policies, network filtering.
5. **`memory`**: Multi-tiered memory (A-MEM, experience, skills, knowledge graph, review, dreams).
6. **`knowledge`**: Knowledge extraction, vault, vault linting.
7. **`runtime`**: Inference engines, `llama-server` client, `model_router`, embedders.
8. **`tools`**: Tool trait, `ToolRegistry`, concrete tools (shell, git, web, file, mcp, etc.).
9. **`agent`**: Agent loop, compaction, task orchestration, fast-path, MAGI panel, DMN loop.
10. **`main`**: CLI entry point and binary assembly.

> **CRITICAL RULE**:
> - Each layer may only import from strictly lower layers (`use crate::<lower_layer>::*`).
> - **Test code (`#[cfg(test)]`) is also bound by DEP-001**. Tests must never import higher-layer concretions; use port traits or lower-layer test mocks (e.g., `MockLlmBackend`).
> - Verified via `cargo test --test structural`.

---

## 5. Build, Test, and Verification Commands

> **Note on Sandbox / Offline Testing**:
> In sandboxed environments or without local ONNX binaries, disable default features to prevent `ort-sys` network downloads.

```bash
# Run unit tests
cargo test --lib --no-default-features --features cli,tree-sitter

# Run architecture, layer order (DEP-001), and file size (SIZE-001) checks
cargo test --test structural --no-default-features --features cli,tree-sitter

# Run Clippy checks
cargo clippy --no-default-features --features cli,tree-sitter -- -D warnings

# Check code formatting
cargo fmt -- --check

# Check capability manifest
cargo run --no-default-features --features cli,tree-sitter -- --manifest
```

---

## 6. Harness Patterns & Mutations

### Defaulted Mutations (Permanently Active)
- **Item 10**: Planning enforcement rules (Lab v6.2 ACCEPT).
- **Item 47**: Intent documentation in `<think>` prior to tool invocation (+0.032).
- **Item 50**: Fallback routing strategy (+0.001).
- **Item 136**: Mandatory file content verification before answering (+0.0157).

### Recent Mutations & Findings
- **265 (Max Context Compaction)**: Pruning budget override under smoke/env conditions; session isolation resets prevent accumulation artifacts.
- **266, 268, 269 (Paired Rejections)**: Dual memory augmentation and aggressive budget tuning rejected under paired testing (Cohen's dz < 0), verifying the paired evidence discipline.

---

## 7. Subagent Orchestration & Delegation

When orchestrating subagents:
1. **Orchestrator Role**: The main agent acts as planner, dispatcher, and synthesizer.
2. **Explicit Models & Skills**: Always declare the target model tier and relevant skill set when delegating.
3. **Separation of Concerns**: The implementing agent (`builder`) must never self-approve. Verification must be performed by a distinct verifier (`qa_verifier` / `code_reviewer`).
4. **Primary Review Optimization**:
   - Standard code and security reviews are first evaluated using lightweight review passes (`agy` / `gemini-3.8-flash-high`) to optimize turnaround and token costs.
   - High-risk changes (cryptography, auth, database schema, breaking architecture changes) escalate to deep reviews.

---

## 8. Tool Selection & Symbol Analysis Rules

The repository contains multi-graph analysis capabilities:
- **Serena Rust Analyzer is INTENTIONALLY DISABLED** in `.serena/project.local.yml` to prevent automated clippy/analyzer rewinds. Do NOT re-enable it without explicit approval.
- Use `better-code-review-graph` or `Graphify` for symbol tracing, impact analysis, and architecture exploration:
  - Code search / purpose query: `better-code-review-graph query (action=search)`
  - Blast radius & impact: `better-code-review-graph query (action=impact)`
  - Callers / definitions: `better-code-review-graph query (action=query)`
  - Broad repository structure: `graphify query`
  - Fallback: ripgrep (`grep_search`), file viewing (`view_file`).

---

## 9. Navigation & Documentation Pointers

- Architectural Overview: [`docs/architecture/overview.md`](docs/architecture/overview.md)
- Layer Linter Rules: [`docs/architecture/module-layer-rules.md`](docs/architecture/module-layer-rules.md)
- Execution Runbook: [`docs/execution/runbook.md`](docs/execution/runbook.md)
- Historical Lab Results: [`docs/quality/lab-history.md`](docs/quality/lab-history.md)
- Architecture Decision Records: [`docs/decisions/README.md`](docs/decisions/README.md)
- Master Index: [`docs/INDEX.md`](docs/INDEX.md)
