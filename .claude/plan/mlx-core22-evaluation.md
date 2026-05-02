# MLX Core 22 Baseline 評価プラン (Session 2026-05-02)

## Task Type
- [x] Backend (process orchestration / benchmark / config swap / docs)
- [ ] Frontend
- [ ] Fullstack

## Multi-Model Synthesis 状況

| Model | 状況 | 採用範囲 |
|-------|------|----------|
| Codex | **完了** (`019de2a7-c34e-7840-8e42-15cd14dcf5ef`、wrapper 経由 codex e) | 技術プラン骨格 / 12 リスク表 / 3-zone decision gate / edge cases / parsing スクリプト |
| Gemini | **完了** (`bq3o70b9w`、direct `gemini -p` 経由) | UX 観点 / TTFT 経時観測 / docs 階層 / メモリ管理示唆 |
| Claude (本体) | 主導 | 統合判断 / llama-server kill 否決判断 / sanitize 手順 / 進捗報告 cadence |

## 主要な合意・分岐点

| 論点 | Codex | Gemini | 採択判断 |
|------|-------|--------|----------|
| llama-server kill 是非 | 保持推奨 (fallback 優先) | kill 推奨 (M2 16GB メモリ圧迫) | **保持** (PID 68103 RSS=9472KB のみ、`-ngl 99` で model は Metal GPU 上、CPU/RSS 影響限定。kill は invasive で benchmark 完走後の比較性に影響) |
| Pre-warmup (smoke 先行) | 不要 (core 自体が degradation test) | 言及なし | **不要** (Codex 採択、純粋な fresh→core 軌跡を測定) |
| Decision gate 構造 | 3-zone (>=0.82 / <=0.78 / 中間) | 2-zone (>0.7571 / <0.7571) | **3-zone** (smoke +0.0969 線形外挿で MLX target=0.85 → 0.82 reproduce / 0.78 lose / 中間 ambiguous) |
| Config sanitize 手法 | backup から copy + edit (live 直接編集禁止) | 軽量サニタイズ | **Codex 方式** (`config.toml.mlx-04-30c-backup` を `config.toml.mlx-core22-${RUN_ID}` にコピー → MCP コメントアウト + sse_chunk_timeout_secs=180 追記) |
| Docs 構造 | 詳細 (raw metrics + timeline + gate table + edge cases) | 高シグナル / 簡潔 | **両方** (`.claude/plan/mlx-core22-result.md` = Codex の詳細、CLAUDE.md 項目 184 = Gemini の高シグナル A/B 表、handoff = Gemini「次に何するか」) |
| 観測のハイレバ指標 | 行別 score / SSE timeout 集計 | TTFT 経時推移 | **両方統合** (毎 task 完了時に grep score、5min 間隔で MLX `/v1/models` レイテンシ計測) |

## Technical Solution (合意プラン)

```
[Phase 0] Pre-flight snapshot (1 min)
  → 現状 commit / config SHA / process state / health check 記録

[Phase 1] Config sanitization (5 min)
  → MLX backup を copy → MCP コメントアウト + sse_chunk_timeout_secs=180 追記
  → live config swap (llama → MLX 設定)

[Phase 2] Benchmark execution (75-90 min, background)
  → BONSAI_BENCH_TIER=core ./target/release/bonsai --lab --lab-experiments 0
  → /tmp/bonsai-llama/mlx-core22.log にリダイレクト
  → run_in_background=true で投入後、即座に進捗観測へ

[Phase 3] Live monitoring (~75 min, no blocking poll)
  → 進捗 5min 間隔で memory pressure / MLX latency / log score 行を観測
  → 異常検知時 (60min CPU stall / mem swap thrash / SSE >10件) は abort + evidence preserve

[Phase 4] Result parsing (5 min)
  → tail で baseline 行抽出 / SSE timeout count / fallback 検出 / TTFT 推移分析

[Phase 5] Decision gate (3-zone judgment)
  → score & 一貫性 & duration & 後半劣化シグナルを総合評価

[Phase 6] Config restore (1 min)
  → llama-server config に確実に戻す (どの outcome でも必須)

[Phase 7] Documentation (15-20 min)
  → mlx-core22-result.md (詳細) + CLAUDE.md 項目 184 (簡潔) + handoff
```

## Implementation Steps

### Phase 0: Pre-flight snapshot

```zsh
cd /Users/keizo/bonsai-agent
RUN_ID="$(date +%Y-%m-%d_%H%M%S)-mlx-core22"
CFG="$HOME/Library/Application Support/bonsai-agent/config.toml"
APPDIR="$HOME/Library/Application Support/bonsai-agent"
LOG="/tmp/bonsai-llama/mlx-core22.log"

# Snapshot
git rev-parse HEAD
shasum -a 256 "$CFG"
ps -p 68103 -o pid,etime,%cpu,%mem,rss,command  # llama-server
lsof -tiTCP:8000 -sTCP:LISTEN | xargs -I{} ps -p {} -o pid,etime,%cpu,%mem,rss,command  # MLX
curl -sS --max-time 5 http://127.0.0.1:8080/v1/models | head -c 200
curl -sS --max-time 5 http://127.0.0.1:8000/v1/models | head -c 200
```

**Pass 条件**: 両 server が 200 応答、MLX PID と uptime 記録、現状 commit `46ae163`。

### Phase 1: Config sanitization & swap

```zsh
# 1.1 Backup current llama config
cp "$CFG" "$APPDIR/config.toml.llama-pre-${RUN_ID}"

# 1.2 Copy MLX backup to working location
cp "$APPDIR/config.toml.mlx-04-30c-backup" "$APPDIR/config.toml.mlx-core22-raw-${RUN_ID}"
```

**Sanitized 仕様** (Edit ツール経由で達成すべき):
- `[model]` セクションに `sse_chunk_timeout_secs = 180` を追記 (MLX cold start 227s 対策、項目 112)
- `[[mcp.servers]]` ブロック (4 行) を全行 `#` でコメントアウト
- `backend = "mlx-lm"` / `server_url = "http://localhost:8000"` / `model_id = "ternary-bonsai-8b"` / `context_length = 65536` は維持

```zsh
# 1.4 Validate sanitized config
diff "$APPDIR/config.toml.mlx-core22-raw-${RUN_ID}" "$APPDIR/config.toml.mlx-core22-sanitized-${RUN_ID}"

# 1.5 Activate MLX config
cp "$APPDIR/config.toml.mlx-core22-sanitized-${RUN_ID}" "$CFG"

# 1.6 Final validation
grep -E '^backend|^server_url|^context_length|^sse_chunk_timeout|^\[\[mcp\.servers\]\]' "$CFG"
# 期待: backend=mlx-lm / server_url=...:8000 / sse_chunk_timeout_secs=180 / 無効 [[mcp.servers]] なし
```

### Phase 2: Benchmark execution

```zsh
mkdir -p /tmp/bonsai-llama
BONSAI_BENCH_TIER=core BONSAI_LOG=info \
  ./target/release/bonsai --lab --lab-experiments 0 \
  > /tmp/bonsai-llama/mlx-core22.log 2>&1 &
BENCH_PID=$!
echo "$BENCH_PID" > "/tmp/bonsai-mlx-core22.pid"
```

**Tool 選択**: `Bash` with `run_in_background: true`、benchmark PID をプラン内記録。`./target/release/bonsai` を直接呼出 (cargo run --release より起動オーバーヘッド削減)。

### Phase 3: Live monitoring

5 分間隔で以下を観測 (`Monitor` tool 不使用、polling sleep で) :

```zsh
# 進捗指標
tail -50 /tmp/bonsai-llama/mlx-core22.log | grep -E "(score|task|baseline|timeout|error)"

# MLX 健全性 (timeout 短く: 5s)
curl -sS --max-time 5 http://127.0.0.1:8000/v1/models > /tmp/mlx-health-check.json && echo "ALIVE" || echo "DOWN/BUSY"

# プロセス状態 (CPU/MEM/uptime)
ps -p $BENCH_PID -o pid,etime,%cpu,%mem,rss
ps -p $(lsof -tiTCP:8000 -sTCP:LISTEN | head -1) -o pid,etime,%cpu,%mem,rss

# システムメモリ (vm_stat で swap pressure 確認)
vm_stat | grep -E "(Swapouts|Pageouts|Pages free)"
```

**異常検知 → abort 判定**:
- `BENCH_PID` の `etime` が 120 分超過、かつ log に新規 task 完了行が 30 分以上ない → hang
- `vm_stat` で `Swapouts` が短時間で >10000 増加 → memory pressure (kill llama-server 検討)
- log に `[fallback]` または `FallbackBackend` 出現 → MLX 失敗で llama-server に切替 = pure MLX 比較不可
- log に `BONSAI_LAB_SMOKE=1` 出現 → tier 設定誤り
- 登録 tool 数 23 出現 → MCP off 失敗

### Phase 4: Result parsing

```zsh
# Baseline 行抽出
grep -E '^\[lab\] ベースライン:' /tmp/bonsai-llama/mlx-core22.log | tail -1

# 期待 format: [lab] ベースライン: score=0.XXXX pass@k=0.XXXX pass_consec=0.XXXX (XXXX.Xs)

# SSE timeout 集計
grep -ciE 'SSE.*timeout|timed out' /tmp/bonsai-llama/mlx-core22.log

# Fallback 検出 (0 でなければ invalid)
grep -ciE '\[fallback\]|FallbackBackend|llama-server' /tmp/bonsai-llama/mlx-core22.log

# Tool 数確認 (9 = MCP off OK / 23 = MCP on NG)
grep -E "tools? registered|登録ツール" /tmp/bonsai-llama/mlx-core22.log | head -3

# Task 別 timeline (degradation 分析用)
grep -E "task [0-9]+|elapsed|duration" /tmp/bonsai-llama/mlx-core22.log > /tmp/mlx-core22-timeline.txt
```

### Phase 5: Decision gate (3-zone)

| Zone | 条件 (全て満たす) | 解釈 |
|------|-------------------|------|
| **A: MLX advantage 再現** | score ≥ 0.82 (smoke +0.097 線形外挿の 96%以上) AND duration ≤ 100 min AND late-run latency cliff なし AND fallback=0 AND MCP off 確認 | MLX が smoke 結果通り core でも勝利。項目 173 (環境劣化) **REJECT**。MLX を quality-preferred backend として推奨 |
| **B: 項目 173 持続** | score ≤ 0.78 OR duration > 120 min OR 後半 SSE/timeout クラスター OR mid-run hang | 環境劣化仮説 **CONFIRM**。llama-server を default 維持。MLX は短時間 task のみ |
| **C: Ambiguous** | 0.78 < score < 0.82 OR 高 score だが duration 大幅退行 OR 劣化シグナルと品質改善の混在 | 1 回 fresh MLX restart 後に再実行で確定 |

**重要**: score 単独で判定しない。MLX が 0.85 でも duration が 2x かつ後半 degrade なら "quality 優位だが operational 劣化" として両論記録。

### Phase 6: Config restore (必須、どの outcome でも)

```zsh
cp "$APPDIR/config.toml.llama-pre-${RUN_ID}" "$CFG"
grep -E '^backend|^server_url|^\[\[mcp\.servers\]\]' "$CFG"
# 期待: backend=llama-server / port 8080 / 無効 [[mcp.servers]] なし
```

### Phase 7: Documentation

#### 7.1 `.claude/plan/mlx-core22-result.md` (詳細、Codex level)

セクション:
1. 計測条件 (date / commit / config SHA / MLX PID-uptime / RUN_ID)
2. 結果メトリクス (raw 表)
3. Phase B1 (llama core 22) 直接比較表
4. Phase C2 (smoke) 線形外挿との一致度
5. Decision gate 評価 (3-zone)
6. Late-run degradation timeline (もし観測されれば)
7. 副次知見 (memory pressure / TTFT 推移 / SSE patterns)
8. 結論 (項目 173 REJECT / CONFIRM / 保留)
9. 次セッション action

#### 7.2 `CLAUDE.md` 項目 184 (簡潔、Gemini level)

```markdown
184. **MLX core 22 評価 (項目 183 派生、項目 173 仮説の最終判定)**: ...結論... (commit `XXXXXXX`)
```

A/B 表のみ:
| 指標 | llama core 22 (B1) | MLX core 22 | Δ |

#### 7.3 `~/.claude/projects/.../session_2026_05_02_handoff.md`

- 完遂サマリー (1 commit: docs CLAUDE.md 184)
- 結論 (項目 173 判定)
- 環境状態 (config 復元済 / process 状態)
- TODO ハンドオフ (継続 DEFER + 派生)

## Key Files

| ファイル | Operation | Description |
|---------|-----------|-------------|
| `~/Library/Application Support/bonsai-agent/config.toml` | Modify (swap) | MLX 用に一時切替、Phase 6 で復元 |
| `~/Library/Application Support/bonsai-agent/config.toml.llama-pre-${RUN_ID}` | Create | llama 設定 backup |
| `~/Library/Application Support/bonsai-agent/config.toml.mlx-core22-sanitized-${RUN_ID}` | Create | sanitize 済 MLX 設定 (MCP off + sse 180) |
| `/tmp/bonsai-llama/mlx-core22.log` | Create | benchmark log |
| `/tmp/mlx-core22-timeline.txt` | Create (Phase 4) | Task 別 timeline |
| `.claude/plan/mlx-core22-result.md` | Create (Phase 7.1) | 結果記録 |
| `CLAUDE.md` | Modify (Phase 7.2) | 項目 184 追記 |
| Production `.rs` files | **NEVER modify** | benchmark のみ、code 変更なし |

## Risks and Mitigation

| Risk | Signal | Mitigation | 解釈 |
|------|--------|------------|------|
| MLX cold start タイムアウト | 初回 request stall, 早期 SSE timeout | `sse_chunk_timeout_secs=180` 追記済、再現時 1 回再実行で warmup 効果除外 | 単発なら artifact、繰返しなら劣化 |
| MLX queue で `/v1/models` busy 応答 | health 5s timeout、log 進行中 | health のみで判定しない、log 進捗を主指標に | 偽陰性 (busy なだけ) |
| MLX hard hang | 30min log 進捗ゼロ、CPU 低位 | ps/lsof/log 保存 → benchmark PID kill → MLX restart 検討 | **項目 173 CONFIRM** |
| MLX mid-run degradation | 後半 task 遅延クラスター、SSE timeout 集中、duration >100 min | 完走させて per-task timing 分析、score 評価と独立に degradation 記録 | **項目 173 部分 CONFIRM** |
| OOM / memory pressure | プロセス消失、`vm_stat` swap 急増 | 即 kill llama-server で M2 unified memory 解放、再実行 | invalid run、再試行 |
| 誤って MCP on | log に 23 tools | abort、Phase 1 sanitize 再確認、再実行 | invalid 比較 |
| FallbackChain 経由 llama に切替 | log に `[fallback]` 出現 | abort、`[fallback_chain]` 設定がないこと再確認、再実行 | invalid pure MLX 比較 |
| Tier 設定漏れ | log に 40 tasks | abort、`BONSAI_BENCH_TIER=core` 確認、再実行 | invalid 比較 |
| Smoke env leak | log に `BONSAI_LAB_SMOKE=1` | abort、shell env reset、再実行 | invalid 比較 |
| Config 復元忘れ | 翌セッションで MLX 残置 | Phase 6 を絶対実行、`trap` でも保証可 | 操作リスク |
| llama-server も同時 kill 必要なケース | M2 swap thrash | mid-run でも llama PID kill 可、benchmark 続行 | memory pressure 緩和 |

## SESSION_IDs (for /ccg:execute)

- **CODEX_SESSION**: `019de2a7-c34e-7840-8e42-15cd14dcf5ef`
- **GEMINI_SESSION**: `bq3o70b9w` (one-shot non-interactive)

## 進行ステータス

- [ ] Phase 0: Pre-flight snapshot
- [ ] Phase 1: Config sanitization & swap
- [ ] Phase 2: Benchmark execution (background)
- [ ] Phase 3: Live monitoring
- [ ] Phase 4: Result parsing
- [ ] Phase 5: Decision gate (3-zone)
- [ ] Phase 6: Config restore
- [ ] Phase 7.1: mlx-core22-result.md
- [ ] Phase 7.2: CLAUDE.md 項目 184
- [ ] Phase 7.3: handoff

## Decision Audit

| 判断 | 根拠 |
|------|------|
| llama-server 保持 (kill しない) | PID 68103 RSS=9472KB のみ、`-ngl 99` で model は Metal、CPU/RSS 影響限定。memory pressure 検出時のみ mid-run kill 検討 |
| Pre-warmup 不要 | core 自体が degradation test (Codex 採択)、純粋 fresh→core 軌跡を測定 |
| 3-zone decision | smoke 線形外挿で MLX target=0.85 → 0.82 は 96% 達成で reproduce 認定、0.78 (llama+0.022) 以下で lose |
| sanitize backup→edit 経由 | live config 直接編集禁止 (Codex)、diff 確認可能、復元安全性 |
| docs 階層 (詳細+簡潔+handoff) | Codex 詳細を `.claude/plan/`、Gemini 簡潔を CLAUDE.md、handoff は次 action 主体 |
| Codex+Gemini 両採択 | Codex は技術 backbone、Gemini は UX/observability 補完、両者で blind spot 削減 |

---

User の承認後、Phase 0 から sequential 実行。Phase 2 投入後は run_in_background で 75-90 min 待機、Phase 3 は 5min 間隔で軽量 polling、Phase 4 以降は完了通知後実行。
