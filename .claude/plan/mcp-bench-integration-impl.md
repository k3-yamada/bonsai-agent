# Plan: MCP-Bench Integration — bonsai-agent MCP capability external validation

> **由来**: arxiv 2508.20453 MCP-Bench (2025-08) "Benchmarking Tool-Using LLM Agents on Real-World Tool Use" の **28 MCP servers / 250 tools / multi-step tasks** 設計を bonsai-agent に統合。bonsai は項目 102 (MCP detach 機構) と項目 182 (MCP detach 効果 core 22 で +0.0492 恒久承認) で MCP 経路を本番運用済だが、**MCP 経路自体の品質を客観計測する benchmark が未整備**。本 plan で既存 `mcp_client.rs` (HTTP/stdio JSON-RPC、自動再接続、Arc<Mutex> 共有接続) を再利用し、mock-first の `mcp_bench_tasks()` 専用 benchmark suite + protocol-aware metric (tool selection precision/recall, round-trip latency) を additive に追加する。
>
> **由来 plan / handoff**: `research_arxiv_2026_05_07.md` 領域 6 (★★★ #9) / CLAUDE.md 項目 102 / 124 / 132-135 / 137 / 180 / 182 / 過去 plan `agentfloor-tier-eval-impl.md` (TDD strict 構造) `beyond-pass1-rdc-vaf-impl.md` (metric 拡張パターン)

## Task Type
- [ ] Frontend
- [x] Backend (`mcp_bench.rs` 新規 + `benchmark.rs` の new tasks set + metric 拡張)
- [ ] Fullstack
- [x] Docs (CLAUDE.md 項目 223 + `memory/mcp_bench_design.md`)
- [ ] Test only

## 1. 背景

### 1.1 MCP-Bench 要点
- **対象**: MCP (Model Context Protocol) 経由のツール使用能力を多 server / 多 tool 環境で計測
- **規模 (論文値)**: 28 MCP servers / 250+ tools / multi-step real-world tasks
- **評価軸**:
  1. **Cross-server tool coordination** — 複数 MCP server を跨いだ tool chain
  2. **Precise parameter control** — JSON Schema に厳密に従った argument 生成
  3. **Planning / reasoning** — multi-step task で誤った tool を選ばないか
  4. **Round-trip latency / error recovery** — JSON-RPC 失敗時の代替経路選択
- **論文 finding**: 1B-8B class small model は cross-server で score < 0.4、parameter precision で誤生成多発、tool selection が hallucination 寄り

### 1.2 bonsai 既存 MCP 実装の現状 (`src/tools/mcp_client.rs` 716 行)
| component | 場所 | 役割 |
|---|---|---|
| `McpServerConfig` | mcp_client.rs:14 | TOML 定義 (name/command/args/url) |
| `McpConnection` | mcp_client.rs:60 | stdio/HTTP transport 抽象、JSON-RPC 2.0 |
| `McpTransport::Stdio` | mcp_client.rs:47 | 子プロセス stdio (BufReader + ChildStdin) |
| `McpTransport::Http` | mcp_client.rs:53 | reqwest::blocking、native-tls |
| `McpToolWrapper` | mcp_client.rs:335 | `Tool` trait 実装、`server:tool` namespace |
| 自動再接続 | mcp_client.rs:384 | stdio プロセス死亡時に `McpConnection::spawn` 再起動 |
| `setup_mcp_server` | main.rs:433 | initialization、`Arc<Mutex<McpConnection>>` 共有 |

✅ **再利用可能**: `McpConnection::spawn` / `list_tools` / `call_tool` / `is_alive` がそのまま使える、本 plan で **mcp_client.rs に変更ゼロ**。

### 1.3 項目 102 / 182 経緯
- **項目 102**: MCP detach 機構 (失敗 server を ToolRegistry から外す circuit-breaker pattern)
- **項目 182**: MCP detach 効果の core 22 ベンチマーク定量化、**score +0.0492 恒久承認** (production default ON)
- ⚠️ **欠落**: 項目 182 は detach の **副作用** (regression) を測ったのみで、MCP 経路 **自体** の品質は default_tasks の 1-2 task (`mcp_basic` 等の Extended) に依存
- 本 plan で **MCP 専用 benchmark suite** を独立計測軸として確立、Lab で MCP-targeted 変異 (例: detach threshold / parameter generation prompt) の効果を可視化

### 1.4 「Scaffolding > Model」原則と整合
- 設計原則 (CLAUDE.md 巻頭): 1bit モデル改善余地は限定的、ハーネス側で底上げ
- MCP-Bench は **harness × protocol** の品質を測る枠組み、bonsai が「MCP detach / 再接続 / parameter 補正のような scaffolding がどの程度 protocol robustness を上げているか」を論文比較可能な形で示す指標
- 副次効果: gpt-4-class 別 backend の MCP-Bench 論文値との external validation (AgentFloor plan #8 と並ぶ第 2 の external check)

## 2. 目的
1. **MCP detach 効果の更なる定量化** — 項目 182 が core 22 全体の +0.0492 を示したのに対し、MCP path 限定で detach ON/OFF の delta を独立計測 (detach の真の効果境界を確定)
2. **Tool selection 精度計測** — multi-server 環境での precision (選んだ tool の正答率) / recall (本来呼ぶべき tool の覆い率) / tool-namespace 誤判定率
3. **論文値との external validation** — bonsai-8B 1bit が gpt-4-class MCP-Bench 公開値に対しどこに位置するか、Lab v17+ の improvement target 設定根拠

### 非目標
- MCP-Bench 28 server / 250 tool 完全コピー (license 不明、外部 server 多数 = test 不安定 / network 依存) → mock-first で内部 fixture
- 既存 `mcp_basic` 等の Extended task 削除 (両軸併存、`mcp_bench_tasks()` 別 method で提供)
- mcp_client.rs 本体への機能追加 (本 plan は **観測のみ**、protocol 改修は別 plan)
- MCP-Bench 別 backend (gpt-4) との並走実行 (本 plan は bonsai-8B baseline 取得のみ、比較は docs PR で論文値引用)
- ACCEPT 判定軸への組込 (informational のみ、composite_score 維持)

## 3. 既存項目との関係
| 項目 | 関係 |
|---|---|
| **102** (MCP detach) | 本 plan の主観測対象。`BONSAI_MCP_DETACH=on/off` で paired t-test 可能化 |
| **124** (MCP マルチサーバー) | mcp_bench で 3 mock server を同時起動、cross-server coordination の前提条件 |
| **132-135** (MCP HTTP/stdio/permission/timeout) | benchmark task が両 transport を網羅 (T-stdio / T-http 各 ≥ 3 task) |
| **137** (MCP tool split policy) | 本 plan では tool 上限 8 を max_tools 引数で MCP-Bench 用に拡張可、env opt-in |
| **180** (MCP namespace `server:tool`) | tool selection 計測で "誤った server 選択" を namespace 解析で検出 |
| **182** (MCP detach 効果定量化) | 本 plan の precondition、direct dependency |
| **172** (Core/Extended Tier) | MCP-Bench は **第 3 軸** (`MCP` 専用 set)、既存 2 軸と直交 |
| **209** (AgentFloor 6-tier、進行中) | T3 Tool Selection / T4 Multi-Step Tool Chain と概念重複あり、§3.1 重複解消で詳述 |
| **200** (Beyond pass@1 RDC/VAF) | MCP path 専用 stability metric を tier_avg と同パターンで `mcp_avg_score` 拡張 |
| **219** (Working Memory Cap) | MCP tool 数増で context overflow が出やすい、`BONSAI_WORKING_CAP_ENABLED` paired 観測候補 |

### 3.1 AgentFloor T3/T4 との重複可能性 (重要)
| 観点 | AgentFloor T3/T4 (項目 209) | MCP-Bench (本 plan) |
|---|---|---|
| 軸 | 「能力梯子」(tool 種別問わず) | 「MCP protocol 経路」限定 |
| tool 種別 | local tools (file_read/shell/grep…) | **MCP namespace tools のみ** (`server:tool`) |
| 計測対象 | tool selection の意思決定能力 | protocol round-trip + namespace 解決 + JSON-RPC error recovery |
| 重複領域 | T3 (tool selection) で ~30% conceptual overlap | T-namespace (mcp 内 server 選択) で吸収 |
| 共存判断 | ✅ 直交軸として共存。AgentFloor は generic scope、MCP-Bench は protocol-specific scope |

→ **両 method 併存**: `agentfloor_tasks()` / `mcp_bench_tasks()` は別 set、`BONSAI_BENCH_LADDER=1` と `BONSAI_BENCH_MCP=1` は **mutually exclusive** (両指定時 MCP 優先、警告 log)。

## 4. 設計

### 4.1 `mcp_bench_tasks()` 新規 benchmark set (15-20 task)
> **task 数決定**: AgentFloor 30 (5/tier × 6 tier) と Lab cycle wall time 制約 (cycle ≤ 90 min @ k=3) のバランスで **18 task** に固定。3 protocol axis × 3 task class × 2 transport 平均で割付。

```rust
// src/agent/mcp_bench.rs (新規 module)

/// MCP-Bench task class (protocol skill axis)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum McpBenchClass {
    /// 単一 MCP server / 単一 tool — 基礎 round-trip
    SingleServerSingleTool,
    /// 単一 server / 複数 tool — namespace + selection
    SingleServerMultiTool,
    /// 複数 server (>=2) を跨ぐ chain — cross-server coordination
    CrossServer,
    /// JSON Schema 厳密 — parameter precision
    ParameterPrecision,
    /// 失敗 server からの recovery — detach + 代替 tool
    ErrorRecovery,
    /// 多 step (>=3) MCP chain — multi-step planning
    MultiStepChain,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Http,
    /// 両方混合 (cross-server で多用)
    Mixed,
}

/// MCP-Bench 専用 BenchmarkTask 拡張 (既存 task は変更ゼロ)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpBenchTask {
    /// 既存 BenchmarkTask 互換 (id / name / input / expected_keywords / max_iterations)
    pub base: BenchmarkTask,
    /// MCP 専用 metadata
    pub mcp_class: McpBenchClass,
    pub transport: McpTransport,
    /// 期待される MCP tool 名 (namespace 込み) — precision/recall 計測
    pub expected_mcp_tools: Vec<String>,  // 例: vec!["mock-fs:read_file", "mock-git:status"]
    /// 必要 server 数 (cross-server 判定)
    pub required_servers: usize,
}

impl BenchmarkSuite {
    /// MCP-Bench 専用 18 task (項目 223、Phase 2 Green 完全実装)
    pub fn mcp_bench_tasks() -> McpBenchSuite { /* ... */ }
}

pub struct McpBenchSuite {
    pub tasks: Vec<McpBenchTask>,
    /// fixture mock servers (Phase 2 Green で 3 server 起動)
    pub fixture_configs: Vec<McpServerConfig>,
}
```

**18 task 内訳**:
| Class | 数 | transport |
|---|---|---|
| SingleServerSingleTool | 3 | stdio×2 / http×1 |
| SingleServerMultiTool | 3 | stdio×2 / http×1 |
| CrossServer | 4 | mixed×3 / stdio×1 |
| ParameterPrecision | 3 | stdio×3 (schema strict) |
| ErrorRecovery | 3 | stdio×2 / http×1 (片方 fail) |
| MultiStepChain | 2 | mixed×2 (3+ step) |
| **合計** | **18** | stdio 10 / http 3 / mixed 5 |

### 4.2 MCP server mock fixture (test 安定性の core)

**設計原則**: 外部 server (npm `@modelcontextprotocol/server-*`) に依存せず、bonsai 内部で **stdio JSON-RPC mock** を実装。`McpConnection::spawn` は `command/args` を `Command::new(...).spawn()` するだけなので、bonsai のテストバイナリ自身を sub-process として再呼び出す pattern を採用 (`#[cfg(test)]` の bin target、または `cargo run --bin mcp-mock --` 経由)。

```rust
// src/agent/mcp_bench/fixture.rs (test-only module)

/// stdio JSON-RPC mock server (test fixture)
///
/// 実装方針: bonsai test 内で std::thread::spawn し、stdin/stdout を pipe で渡す。
/// `McpConnection::spawn(config)` は外部プロセス前提なので、本 plan では別 binary
/// (`mcp-mock-fs` / `mcp-mock-git` / `mcp-mock-flaky`) を `[[bin]]` で Cargo.toml に
/// 登録し、`Command::new("./target/debug/mcp-mock-fs")` で起動可能にする。
///
/// 起動 binary の本体ロジック (Phase 2 Green で実装):
/// - `McpMockServer::run_stdio()` が stdin から JSON-RPC 行を読み、tools/list と
///   tools/call の固定レスポンスを返す。
/// - `McpMockBehavior` で flaky / slow / param-strict 等の test 用挙動を選択。
pub struct McpMockServer {
    pub name: String,
    pub tools: Vec<McpMockTool>,
    pub behavior: McpMockBehavior,
}

#[derive(Debug, Clone)]
pub enum McpMockBehavior {
    /// 通常応答
    Normal,
    /// N 回目で必ず失敗 (ErrorRecovery 用)
    FailAfter(usize),
    /// 起動後 N ms 遅延 (latency 計測用)
    SlowResponse(u64),
    /// JSON Schema 不一致 args で error 返却 (ParameterPrecision 用)
    ParamStrict,
}

#[derive(Debug, Clone)]
pub struct McpMockTool {
    pub name: String,
    pub schema: serde_json::Value,
    pub canned_response: String,
}
```

**3 fixture mock servers** (Cargo.toml `[[bin]]` 3 件追加):
1. `mcp-mock-fs` — read_file / write_file / list_dir (5 tool)
2. `mcp-mock-git` — status / log / diff (3 tool)
3. `mcp-mock-flaky` — fetch_url / parse_json (2 tool、`FailAfter(2)` 挙動で recovery test)

**理由**: 既存 mcp_client.rs を変更ゼロで使う最小コスト経路。`#[cfg(test)]` 内でも `Command::new` は使えるので、test build artifact を `target/debug/mcp-mock-*` から自動 spawn。

### 4.3 Tool selection metric

```rust
// src/agent/mcp_bench/metric.rs (新規 module)

/// MCP tool selection の precision/recall (1 task あたり)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpToolSelectionMetric {
    /// 実際に呼ばれた MCP tool (namespace 込み、order 保持)
    pub called_tools: Vec<String>,
    /// 期待された MCP tool set
    pub expected_tools: Vec<String>,
    /// True positive — called ∩ expected
    pub tp: usize,
    /// False positive — called \ expected (誤選択)
    pub fp: usize,
    /// False negative — expected \ called (取り逃し)
    pub fn_count: usize,
    /// namespace 誤判定 (例: `mock-fs:read_file` を期待して `mock-git:read_file` 呼出)
    pub namespace_misroute: usize,
}

impl McpToolSelectionMetric {
    pub fn precision(&self) -> Option<f64> {
        let denom = self.tp + self.fp;
        if denom == 0 { None } else { Some(self.tp as f64 / denom as f64) }
    }

    pub fn recall(&self) -> Option<f64> {
        let denom = self.tp + self.fn_count;
        if denom == 0 { None } else { Some(self.tp as f64 / denom as f64) }
    }

    pub fn f1(&self) -> Option<f64> {
        let p = self.precision()?;
        let r = self.recall()?;
        if p + r == 0.0 { Some(0.0) } else { Some(2.0 * p * r / (p + r)) }
    }
}

/// MCP round-trip latency (per task aggregate)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpLatencyMetric {
    pub call_count: usize,
    pub total_ms: u64,
    pub max_ms: u64,
    /// JSON-RPC error 回数
    pub rpc_errors: usize,
    /// 自動再接続発動回数
    pub reconnects: usize,
}

impl McpLatencyMetric {
    pub fn mean_ms(&self) -> Option<f64> {
        if self.call_count == 0 { None }
        else { Some(self.total_ms as f64 / self.call_count as f64) }
    }
}
```

### 4.4 `BONSAI_BENCH_MCP=1` env 起動

```rust
// src/agent/experiment.rs (run_experiment_loop 冒頭)
let mcp_mode = std::env::var("BONSAI_BENCH_MCP")
    .map(|v| v == "1" || v == "on" || v == "true")
    .unwrap_or(false);

let suite = if mcp_mode {
    BenchmarkSuite::from_mcp_bench(BenchmarkSuite::mcp_bench_tasks())
} else if std::env::var("BONSAI_BENCH_LADDER").map(|v| v == "1").unwrap_or(false) {
    BenchmarkSuite::agentfloor_tasks()
} else if std::env::var("BONSAI_BENCH_TIER").as_deref() == Ok("core") {
    BenchmarkSuite::core_tasks()
} else {
    BenchmarkSuite::default_tasks()
};
```

**排他性保証**: `BONSAI_BENCH_MCP=1` と `BONSAI_BENCH_LADDER=1` 同時指定時、MCP 優先 + warning log 出力。

### 4.5 既存 ToolRegistry / mcp_client 経由 (重複ゼロ)
- 本 plan で追加する production code は **3 module + 3 binary**:
  - `src/agent/mcp_bench/mod.rs` (40-50 行: `McpBenchTask` / `McpBenchClass` / `McpTransport` / `mcp_bench_tasks()`)
  - `src/agent/mcp_bench/metric.rs` (60-80 行: `McpToolSelectionMetric` / `McpLatencyMetric`)
  - `src/agent/mcp_bench/fixture.rs` (`#[cfg(test)]` のみ、120-150 行)
  - `src/bin/mcp-mock-fs.rs` / `src/bin/mcp-mock-git.rs` / `src/bin/mcp-mock-flaky.rs` (各 60-80 行)
- ❌ **mcp_client.rs / config.rs / main.rs に変更ゼロ** (additive 100%)
- ✅ ToolRegistry には MCP fixture を benchmark setup 時に追加するのみ、既存 register API を再利用

### 4.6 `MultiRunBenchmarkResult` 拡張 (additive)
```rust
pub struct MultiRunBenchmarkResult {
    // 既存 fields (task_scores, duration_secs, core_avg_score, extended_avg_score, tier_avg_scores...)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_avg_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_avg_precision: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_avg_recall: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_avg_f1: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_mean_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_class_breakdown: Option<[Option<f64>; 6]>,  // 6 McpBenchClass 別 score
}
```

### 4.7 Lab summary 出力例
```
[INFO][lab.mcp-bench] MCP-Bench summary (cycle 3, baseline):
  task_count        : 18 / k=3 = 54 run
  composite_score   : 0.612
  precision         : 0.78  (called/(called+misroute))
  recall            : 0.71
  f1                : 0.745
  mean_latency_ms   : 12.4 (max 89.0)
  rpc_errors        : 4 / 54 (7.4%)
  reconnects        : 1 / 54 (1.9%)
  class_breakdown:
    SingleServer-1tool : 0.85  (3 task)  <- 強
    SingleServer-Nrool : 0.72  (3 task)
    CrossServer        : 0.51  (4 task)  <- 攻めるべき
    ParameterPrecision : 0.43  (3 task)  <- 1bit JSON Schema 弱点典型
    ErrorRecovery      : 0.69  (3 task)
    MultiStepChain     : 0.38  (2 task)  <- ceiling
  paper_delta (vs MCP-Bench gpt-4-class):
    composite           : 0.612 vs ~0.82 (delta -0.21)
    cross_server        : 0.51  vs ~0.78 (delta -0.27)
```

### 4.8 SQLite + TSV 永続化 (依存順序明記)
> ⚠️ **依存順序**: AgentFloor plan (項目 209、`agentfloor-tier-eval-impl.md`) が **SQLite V11 + TSV 21 列** を確保予定。本 plan は AgentFloor merge 後に **V12 + TSV 27 列** で乗る。AgentFloor 未 merge で本 plan を先行する場合は V11 を本 plan が確保 (`memory/mcp_bench_design.md` で migration 順序明記)。

**SQLite (AgentFloor 後の場合 V11 -> V12)**:
```sql
ALTER TABLE experiments ADD COLUMN mcp_score REAL;
ALTER TABLE experiments ADD COLUMN mcp_precision REAL;
ALTER TABLE experiments ADD COLUMN mcp_recall REAL;
ALTER TABLE experiments ADD COLUMN mcp_f1 REAL;
ALTER TABLE experiments ADD COLUMN mcp_latency_ms REAL;
ALTER TABLE experiments ADD COLUMN mcp_class_breakdown TEXT;  -- JSON 文字列で 6 値
```

**TSV 21 -> 27 列**: 末尾 `mcp_score / mcp_precision / mcp_recall / mcp_f1 / mcp_latency_ms / mcp_class_breakdown` 6 列追加 (NaN は `-`)。

## 5. TDD strict 5 phase

### Phase 1 — Red
新規 test 8 件 (mcp_bench/mod.rs / metric.rs / fixture.rs / experiment.rs):

1. `test_mcp_bench_tasks_18_count` — `mcp_bench_tasks().tasks.len() == 18`、各 class 規定数
2. `test_mcp_bench_class_distribution` — 6 class 全 >= 1 task、transport stdio:http:mixed = 10:3:5
3. `test_mcp_tool_selection_metric_precision_recall` — TP=2/FP=1/FN=1 -> precision=2/3, recall=2/3, f1≈0.667
4. `test_mcp_tool_selection_namespace_misroute` — `mock-fs:read_file` 期待で `mock-git:read_file` 呼出時 misroute=1
5. `test_mcp_latency_metric_mean` — call_count=4 / total=80ms -> mean=20.0ms
6. `test_mcp_mock_server_normal_response` — `McpMockServer { Normal }` で `tools/list` JSON-RPC 行返却
7. `test_mcp_mock_server_fail_after` — `FailAfter(2)` で 1,2 回成功 / 3 回目で error
8. `test_mcp_bench_env_takes_precedence_over_ladder` — `BONSAI_BENCH_MCP=1` + `BONSAI_BENCH_LADDER=1` で MCP 選択 + warn log

期待: compile error or 全 fail で Red 確認。

### Phase 2 — Green
1. `McpBenchClass` / `McpTransport` enum + `McpBenchTask` struct -> test 1, 2 pass
2. `McpToolSelectionMetric` + precision/recall/f1 -> test 3, 4 pass
3. `McpLatencyMetric` + mean_ms -> test 5 pass
4. `McpMockServer` + 3 binary (`src/bin/mcp-mock-{fs,git,flaky}.rs`) -> test 6, 7 pass
5. `mcp_bench_tasks()` 実装 (18 task definition) -> test 1 pass
6. `run_experiment_loop` env precedence -> test 8 pass
7. `MultiRunBenchmarkResult::mcp_avg_*` 追加 + `run_k` 内で MCP task のみ集計

期待: 既存 1150 + 新規 8 = **1158 passed** / clippy 0 / fmt 0

### Phase 3 — Refactor
- `McpToolSelectionMetric` aggregate helper (`from_session_log` で session の tool_calls から自動抽出)
- `weakest_class()` / `paper_delta_map()` helper (AgentFloor `weakest_tier` と対称)
- docstring 整備 (項目 223 参照、MCP-Bench 由来明記)
- `BONSAI_BENCH_MCP` env 読込 helper を `BONSAI_BENCH_LADDER` と並列に整理

### Phase 4 — Smoke 検証 (3 段)
```bash
# G-4a: 既存 default_tasks 経路 (後方互換、env 未設定)
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: smoke 7 task 実行、1150 pass 維持、mcp_avg_score=None で TSV 列空表現

# G-4b: MCP-Bench 18 task sanity
cargo build --release --bin bonsai --bin mcp-mock-fs --bin mcp-mock-git --bin mcp-mock-flaky
BONSAI_BENCH_MCP=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0
# 期待: 18 task / k=3 = 54 run で 25-40 min、全 6 class >= 1 task 完走、mcp_avg_score Some(>0.0)

# G-4c: 完全 baseline (1 cycle のみ)
BONSAI_BENCH_MCP=1 ./target/release/bonsai --lab --lab-experiments 1
# 期待: log [INFO][lab.mcp-bench] summary 出力、composite_score >= 0.30 (1bit + MCP overhead 下限)
```

判定:
- ✅ G-4a: 既存経路 1150 pass 維持
- ✅ G-4b: 18 task smoke で 6 class 全 >= 1 valid score、`mcp_class_breakdown` 6 値 Some
- ✅ G-4c: composite_score >= 0.30 + precision >= 0.50 + JSON-RPC error rate <= 20%

### Phase 5 — Commit + handoff + CLAUDE.md 項目 223
5 commits:
1. `test(mcp-bench): Phase 1 Red — 18 task suite + metric + mock fixture test`
2. `feat(mcp-bench): Phase 2 Green — McpBenchTask + Selection/Latency metric + 3 mock binary`
3. `refactor(mcp-bench): Phase 3 — weakest_class helper + env precedence + docstring`
4. `feat(experiment): Phase 4 — Lab summary mcp class breakdown + V11/V12 + TSV 27 列`
5. `docs(claude.md): 項目 223 — MCP-Bench 統合完遂 + smoke G-4 PASS`

## 6. API 影響
| API | 変更 | 後方互換 |
|---|---|---|
| `McpBenchTask` / `McpBenchClass` / `McpTransport` enum | 新規 | — |
| `BenchmarkSuite::mcp_bench_tasks()` | 新規 method | — |
| `McpBenchSuite` 構造体 | 新規 (BenchmarkSuite と並列) | — |
| `McpToolSelectionMetric` / `McpLatencyMetric` | 新規 | — |
| `MultiRunBenchmarkResult` | 6 field 追加 (mcp_avg_*) | ✅ serde default + skip_if_none |
| `Experiment` (experiment_log) | mcp_score/precision/recall/f1/latency/class_breakdown 6 Option | ✅ default + skip |
| SQLite | V12 (AgentFloor merge 後) または V11 (先行時) | ✅ additive ALTER TABLE |
| TSV | 21 -> 27 列 (末尾追加) | ⚠️ header 駆動 reader OK |
| env | `BONSAI_BENCH_MCP=1` 新規 | ✅ default 未設定で既存挙動 |
| Cargo.toml | `[[bin]]` 3 件追加 (mcp-mock-{fs,git,flaky}) | ✅ 既存 bonsai bin に影響なし |
| `mcp_client.rs` | **変更ゼロ** | ✅ 100% 再利用 |
| `main.rs` | **変更ゼロ** | ✅ 100% additive |

**signature 変更ゼロ** — 全 additive、項目 205 のような必須化はなし。

## 7. Risks / Mitigations
| # | Risk | 影響 | Mitigation |
|---|---|---|---|
| R1 | mock fixture binary が test build artifact 依存 (`./target/debug/mcp-mock-fs` 起動経路) で CI/release で path 解決失敗 | smoke G-4b/c フェイル | (i) `env!("CARGO_BIN_EXE_mcp-mock-fs")` を test 内で解決 (ii) Phase 4 G-4b 直前に `cargo build --bin mcp-mock-*` 明示 (iii) Quick Start に build 手順を必須記載 |
| R2 | 18 task × k=3 = 54 run + MCP round-trip overhead で Lab cycle 時間 +30-50% 膨張 | 運用負担 | (i) `BONSAI_BENCH_MCP=1` env で opt-in (ii) 初回 baseline は k=1 で 18 run 圧縮可 (iii) max_iterations <= 6 を MCP task 全体に強制 |
| R3 | MCP 経路で `expected_mcp_tools` 判定が namespace 解析依存、`server:tool` format 崩れ時の誤検出 | precision/recall 誤計測 | (i) `McpToolWrapper::name()` (mcp_client.rs:362) 経路で必ず `server:tool` format、unit test で format invariant assert (ii) Phase 1 test 4 で namespace_misroute 専用 test |
| R4 | flaky mock (`FailAfter(2)`) のタイミング非決定性で test flake | CI 不安定 | (i) `McpMockBehavior::FailAfter` は call_count atomic counter で deterministic (ii) Phase 1 test 7 で 3 回連続呼出 assert (iii) 1bit variance 由来の flake は `#[ignore]` で smoke 隔離 |
| R5 | `Command::new("./target/debug/mcp-mock-fs")` の cwd 依存で test 失敗 (CARGO_MANIFEST_DIR 問題) | dev / CI 双方で test 落ち | (i) `cargo test` 起動時 `env!("CARGO_BIN_EXE_*")` を `McpServerConfig.command` に注入 (ii) integration test は `#[ignore]` 化、unit test は schema/JSON-RPC 片側のみ検証 |
| R6 | 項目 209 (AgentFloor) と SQLite V 番号競合 | migration 失敗 | (i) **本 plan は AgentFloor merge 後に乗る** dependency 順序、handoff で明記 (ii) AgentFloor 未 merge で本 plan 先行する場合は V11 を本 plan が確保 (iii) Phase 1 test に `PRAGMA user_version` 確認 |
| R7 | MCP-Bench 論文値 (gpt-4-class score 0.82) の引用妥当性、bonsai-8B 1bit との直接比較は category mismatch | external validation の説得力低下 | (i) docs `memory/mcp_bench_design.md` で「論文値は capacity ceiling、bonsai-8B との absolute delta は意味なく、relative trajectory (Lab v17 -> v18 改善幅) を主軸」と明記 (ii) Phase 5 commit message にも同旨記載 |
| R8 | 1bit variance で 18 task / 6 class の class_breakdown が k=3 では noisy | class 別計測信頼性低下 | (i) k=3 -> k=5 増 option (ii) 項目 200 RDC/VAF と 2D で見る (iii) Phase 4 G-4c で「最低 6 class 全 >= 0.20」のみ確認、絶対値は informational |

## 8. Quality Gates
- **G-1 Phase 1 Red**: 8 新規 test compile error or 全 fail
- **G-2 Phase 2 Green**: 8 新規 test PASS + 1150 維持 = **1158 passed** + clippy 0 + fmt 0
- **G-3 Phase 3 Refactor**: docstring 完備 + helper 追加 + 既存 test 退行ゼロ
- **G-4 Phase 4 Smoke 3 段**:
  - G-4a: 既存経路 1150 pass 維持 (env 未設定で `mcp_*` field None)
  - G-4b: 18 task smoke で 6 class 全 >= 1 valid score
  - G-4c: 18 task k=3 baseline で composite_score >= 0.30 + precision >= 0.50 + rpc_error_rate <= 20%
- **G-5 Final**: MCP-Bench summary が Lab log 出力 + V11/V12 + TSV 27 列 + handoff 起票 + CLAUDE.md 項目 223
- **G-6 (informational)**: paper_delta が `memory/mcp_bench_design.md` に記録、Lab v17+ improvement target に活用可能

## 9. 完了条件
1. ✅ `McpBenchClass` / `McpTransport` enum + `McpBenchTask` struct 追加
2. ✅ `mcp_bench_tasks()` 18 task 完全実装 (6 class × 各 >= 2 task、transport stdio:http:mixed = 10:3:5)
3. ✅ `McpToolSelectionMetric` (precision/recall/f1/namespace_misroute) + `McpLatencyMetric` 実装
4. ✅ 3 mock binary (`mcp-mock-fs` / `mcp-mock-git` / `mcp-mock-flaky`) 実装、`McpMockBehavior` 4 variant
5. ✅ `BONSAI_BENCH_MCP=1` env で Lab 起動可、AgentFloor / Tier env と排他処理
6. ✅ SQLite migration (V11 or V12) + TSV 27 列 (依存順序 docs 反映)
7. ✅ Lab summary に MCP class breakdown (4.7 形式)
8. ✅ smoke G-4a/b/c 全 PASS
9. ✅ 1158+ passed 維持 / clippy 0 / fmt 0
10. ✅ `mcp_client.rs` / `main.rs` 変更ゼロ確証 (`git diff --stat src/tools/mcp_client.rs src/main.rs` で 0 lines)
11. ✅ CLAUDE.md 項目 223 + `memory/mcp_bench_design.md` 起票

## 10. 見積もり
| Phase | 内容 | 時間 |
|-------|------|------|
| Phase 1 | Red — 8 test 追加 (mod.rs / metric.rs / fixture.rs / experiment.rs) | 1.0h |
| Phase 2 | Green — enum + 18 task + 2 metric + 3 mock binary + run_k 集計 | 4.5h |
| Phase 3 | Refactor — helper + env precedence + docstring | 1.0h |
| Phase 4 | Smoke 3 段 (うち c は 30-40 min × 1 cycle 実機) | 2.5h (実機 wall 1.5h) |
| Phase 5 | Commit + handoff + CLAUDE.md 項目 + memory/mcp_bench_design.md | 1.5h |
| Buffer | mock binary path 解決 (R1/R5) / SQLite migration debug (R6) | 1.5h |
| **合計** | | **~12h ≈ 1-1.5 day** |

## 11. Quick Start
```bash
# 0. 既存 caller 全網羅 + 依存確認
rtk grep -rn "mcp_bench\|McpBench" src/  # 期待 0 件
rtk grep -rn "BONSAI_BENCH_MCP" src/      # 期待 0 件
rtk grep -rn "BONSAI_BENCH_LADDER\|BONSAI_BENCH_TIER" src/  # AgentFloor / 項目 172 既存確認
rtk grep -n "agentfloor_tasks\|core_tasks\|extended_tasks" src/agent/benchmark.rs

# 1. Phase 1 Red
$EDITOR src/agent/mcp_bench/mod.rs       # 新規 (Cargo.toml mod 登録 + lib.rs pub mod)
$EDITOR src/agent/mcp_bench/metric.rs    # 新規
$EDITOR src/agent/mcp_bench/fixture.rs   # 新規 (#[cfg(test)] のみ)
$EDITOR Cargo.toml                       # [[bin]] 3 件追加 (mcp-mock-{fs,git,flaky})
$EDITOR src/bin/mcp-mock-fs.rs           # 新規 (skeleton)
rtk cargo test --lib mcp_bench           # compile error or fail

# 2. Phase 2 Green
$EDITOR src/agent/mcp_bench/mod.rs       # 18 task 完全定義
$EDITOR src/agent/mcp_bench/metric.rs    # precision/recall/f1
$EDITOR src/bin/mcp-mock-{fs,git,flaky}.rs  # JSON-RPC stdio loop
$EDITOR src/agent/benchmark.rs           # MultiRunBenchmarkResult mcp_avg_* 追加
$EDITOR src/agent/experiment.rs          # env precedence
$EDITOR src/db/migrate.rs                # V11 or V12 (AgentFloor 状況で分岐)
$EDITOR src/agent/experiment_log.rs      # Experiment 6 field
rtk cargo build --bin mcp-mock-fs --bin mcp-mock-git --bin mcp-mock-flaky
rtk cargo test --lib                     # 1158 passed

# 3. Phase 3 Refactor
$EDITOR src/agent/mcp_bench/mod.rs       # weakest_class / paper_delta_map / docstring
$EDITOR src/agent/experiment.rs          # MCP 環境警告 log

# 4. Phase 4 Smoke 3 段
rtk cargo build --release
BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4a
BONSAI_BENCH_MCP=1 BONSAI_LAB_SMOKE=1 ./target/release/bonsai --lab --lab-experiments 0  # G-4b
BONSAI_BENCH_MCP=1 ./target/release/bonsai --lab --lab-experiments 1  # G-4c (30-40 min)
grep "lab.mcp-bench" /tmp/bonsai_*.log

# 5. Commit + handoff + CLAUDE.md 項目 223
```

## 12. 参考
- arxiv 2508.20453 MCP-Bench (https://arxiv.org/pdf/2508.20453) — 28 server / 250 tool / multi-step
- arxiv 2512.24565 MCPAgentBench (https://arxiv.org/abs/2512.24565) — real-world MCP task 補足
- arxiv 2508.16260 MCPVerse (https://arxiv.org/html/2508.16260v2) — 550+ tool 巨大 space (★ 低、tool 上限前提が異なる)
- 既存 implementation:
  - `src/tools/mcp_client.rs:14` `McpServerConfig` / `:60` `McpConnection` / `:335` `McpToolWrapper`
  - `src/main.rs:433` `setup_mcp_server` (再利用 reference)
  - `src/agent/benchmark.rs:600` `BenchmarkSuite::smoke_tasks()` / `:620` `core_tasks()` / `:649` `agentfloor_tasks()` stub / `:654` `default_tasks()`
- 過去 plan:
  - `agentfloor-tier-eval-impl.md` (TDD strict 5 phase 構造、tier metric 拡張パターン、SQLite V10->V11 migration)
  - `beyond-pass1-rdc-vaf-impl.md` (MultiRunTaskScore additive 拡張パターン、TSV 列追加)
  - `agenther-runtime-integration-impl.md` (Lab hook 配線パターン、observable invariance)
- CLAUDE.md 項目 102 (MCP detach) / 124 (multi-server) / 132-135 (transport/permission) / 137 (tool split) / 180 (namespace) / 182 (detach 効果定量化) / 172 (Core/Extended) / 209 (AgentFloor、進行中)
- `research_arxiv_2026_05_07.md` 領域 6 #9 ★★★ (MCP-Bench external validation)
- 派生候補:
  - Lab v18 MCP-targeted 変異 (detach threshold / parameter prompt) の paired t-test
  - MCPAgentBench / MCPVerse 部分採用 (tool 上限拡張議論と sequence)
  - external backend (gpt-4-class) との並走実行 (別 plan、license / API key 必要)
