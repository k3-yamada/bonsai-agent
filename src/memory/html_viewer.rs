//! オフライン単一 HTML 力学グラフビューア生成器
//!
//! 第7章「可視化層への投資」に基づき、外部 CDN やネットワーク依存を一切排除した
//! 単一自己完結型 HTML ファイル（SVG + 2D 力学シミュレーション + サイドバー詳細表示）を生成する。

use anyhow::Result;

use crate::memory::export::{GraphSnapshot, export_memory_graph};
use crate::memory::store::MemoryStore;

/// メモリストアからエクスポートし、スタンドアロン HTML 文字列を生成する。
pub fn export_and_render_html(store: &MemoryStore, filter_privacy: bool) -> Result<String> {
    let snapshot = export_memory_graph(store, filter_privacy)?;
    Ok(generate_standalone_html(&snapshot))
}

/// GraphSnapshot を埋め込んだ単一自己完結型 HTML 文字列を生成する。
pub fn generate_standalone_html(snapshot: &GraphSnapshot) -> String {
    let graph_json = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_string());
    // XSS 防止のため </script> をエスケープ
    let safe_json = graph_json.replace("</script>", "<\\/script>");

    format!(
        r##"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="UTF-8">
<title>bonsai-agent Memory &amp; Knowledge Graph</title>
<style>
  * {{ box-sizing: border-box; margin: 0; padding: 0; }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    background: #0f172a;
    color: #e2e8f0;
    overflow: hidden;
    height: 100vh;
    display: flex;
    flex-direction: column;
  }}
  header {{
    background: #1e293b;
    padding: 10px 20px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    border-bottom: 1px solid #334155;
    z-index: 10;
  }}
  .title {{
    font-weight: 700;
    font-size: 1.1rem;
    color: #38bdf8;
    display: flex;
    align-items: center;
    gap: 8px;
  }}
  .stats {{
    font-size: 0.85rem;
    color: #94a3b8;
  }}
  .search-box {{
    background: #0f172a;
    border: 1px solid #334155;
    color: #f8fafc;
    padding: 6px 12px;
    border-radius: 6px;
    font-size: 0.9rem;
    width: 240px;
  }}
  .search-box:focus {{
    outline: none;
    border-color: #38bdf8;
  }}
  .container {{
    position: relative;
    flex: 1;
    display: flex;
  }}
  svg {{
    flex: 1;
    width: 100%;
    height: 100%;
    cursor: grab;
  }}
  svg:active {{
    cursor: grabbing;
  }}
  .sidebar {{
    width: 320px;
    background: #1e293b;
    border-left: 1px solid #334155;
    padding: 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
    box-shadow: -2px 0 8px rgba(0,0,0,0.3);
    transition: transform 0.2s ease;
  }}
  .sidebar.hidden {{
    transform: translateX(100%);
    position: absolute;
    right: 0;
    height: 100%;
  }}
  .badge {{
    display: inline-block;
    padding: 3px 8px;
    border-radius: 4px;
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
  }}
  .badge-memory {{ background: #0284c7; color: white; }}
  .badge-kg {{ background: #10b981; color: white; }}
  .detail-card {{
    background: #0f172a;
    padding: 12px;
    border-radius: 6px;
    border: 1px solid #334155;
    font-size: 0.88rem;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 350px;
    overflow-y: auto;
  }}
  .links-list {{
    list-style: none;
    font-size: 0.82rem;
  }}
  .links-list li {{
    padding: 4px 0;
    border-bottom: 1px solid #334155;
    color: #cbd5e1;
  }}
  .controls {{
    position: absolute;
    bottom: 20px;
    left: 20px;
    background: #1e293b;
    padding: 8px 12px;
    border-radius: 8px;
    border: 1px solid #334155;
    display: flex;
    gap: 8px;
    z-index: 10;
  }}
  .btn {{
    background: #334155;
    color: #f8fafc;
    border: none;
    padding: 6px 12px;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }}
  .btn:hover {{ background: #475569; }}
</style>
</head>
<body>

<header>
  <div class="title">
    <span>🌱 bonsai-agent</span>
    <span>Memory Dynamics Graph</span>
  </div>
  <div class="stats" id="stats-header">Loading...</div>
  <input type="text" id="search" class="search-box" placeholder="Search node / tag...">
</header>

<div class="container">
  <svg id="canvas"></svg>

  <aside class="sidebar" id="sidebar">
    <div style="display:flex; justify-content:space-between; align-items:center;">
      <span id="detail-category" class="badge badge-memory">Category</span>
      <button class="btn" style="padding:2px 8px;" onclick="closeSidebar()">×</button>
    </div>
    <h3 id="detail-title" style="font-size:1.05rem; word-break:break-word;">Select a node</h3>
    <div id="detail-tags" style="display:flex; gap:4px; flex-wrap:wrap;"></div>
    <div>
      <div style="font-size:0.75rem; color:#94a3b8; margin-bottom:4px;">DETAILS</div>
      <div class="detail-card" id="detail-body">Click any node on the graph to inspect memories and relational insights.</div>
    </div>
    <div>
      <div style="font-size:0.75rem; color:#94a3b8; margin-bottom:4px;">CONNECTED RELATIONS</div>
      <ul class="links-list" id="detail-links">
        <li>No node selected</li>
      </ul>
    </div>
  </aside>

  <div class="controls">
    <button class="btn" onclick="resetZoom()">Reset View</button>
    <button class="btn" onclick="toggleSimulation()" id="sim-btn">Pause Sim</button>
  </div>
</div>

<script>
const DATA = {safe_json};

let width = window.innerWidth - (document.getElementById('sidebar').classList.contains('hidden') ? 0 : 320);
let height = window.innerHeight - 50;

const svg = document.getElementById('canvas');
svg.setAttribute('viewBox', `0 0 ${{width}} ${{height}}`);

// メトリクス初期化
document.getElementById('stats-header').textContent = 
  `Memories: ${{DATA.total_memories || 0}} | Entities: ${{DATA.total_entities || 0}} | Edges: ${{DATA.edges.length || 0}} | Exported: ${{DATA.exported_at ? DATA.exported_at.substring(0, 19) : '-'}}`;

// ノード & エッジ データセットの初期位置割り当て
const nodes = (DATA.nodes || []).map((n, i) => ({{
  ...n,
  x: width / 2 + (Math.random() - 0.5) * 400,
  y: height / 2 + (Math.random() - 0.5) * 400,
  vx: 0,
  vy: 0,
  radius: Math.max(7, Math.min(22, 6 * (n.weight || 1.0)))
}}));

const nodeMap = new Map();
nodes.forEach(n => nodeMap.set(n.id, n));

const edges = (DATA.edges || []).map(e => ({{
  ...e,
  sourceNode: nodeMap.get(e.source),
  targetNode: nodeMap.get(e.target)
}})).filter(e => e.sourceNode && e.targetNode);

// 描画グループ構築
const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
svg.appendChild(g);

let zoomScale = 1.0;
let panX = 0;
let panY = 0;

function updateTransform() {{
  g.setAttribute("transform", `translate(${{panX}}, ${{panY}}) scale(${{zoomScale}})`);
}}

// エッジ描画要素の作成
const edgeElements = edges.map(e => {{
  const line = document.createElementNS("http://www.w3.org/2000/svg", "line");
  line.setAttribute("stroke", "#334155");
  line.setAttribute("stroke-width", "1.5");
  line.setAttribute("stroke-opacity", "0.6");
  g.appendChild(line);
  return {{ el: line, data: e }};
}});

// ノード描画要素の作成
const nodeElements = nodes.map(n => {{
  const circle = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  circle.setAttribute("r", n.radius);
  const isMemory = n.id.startsWith("mem_");
  circle.setAttribute("fill", isMemory ? "#38bdf8" : "#34d399");
  circle.setAttribute("stroke", "#0f172a");
  circle.setAttribute("stroke-width", "2");
  circle.style.cursor = "pointer";

  // タイトルツールチップ
  const title = document.createElementNS("http://www.w3.org/2000/svg", "title");
  title.textContent = `[${{n.category}}] ${{n.label}}`;
  circle.appendChild(title);

  // クリックイベント
  circle.addEventListener("click", (evt) => {{
    evt.stopPropagation();
    selectNode(n);
  }});

  // ドラッグ操作
  circle.addEventListener("mousedown", (evt) => {{
    draggedNode = n;
    evt.stopPropagation();
  }});

  g.appendChild(circle);
  return {{ el: circle, data: n }};
}});

// ラベル描画（重要ノードのみ）
const labelElements = nodes.filter(n => n.weight > 1.2 || nodes.length < 50).map(n => {{
  const text = document.createElementNS("http://www.w3.org/2000/svg", "text");
  text.textContent = n.label;
  text.setAttribute("fill", "#94a3b8");
  text.setAttribute("font-size", "10px");
  text.setAttribute("text-anchor", "middle");
  text.setAttribute("pointer-events", "none");
  g.appendChild(text);
  return {{ el: text, data: n }};
}});

// 力学シミュレーションステップ
let simRunning = true;
function stepSimulation() {{
  if (!simRunning && !draggedNode) return;

  const kRepulse = 800;
  const kSpring = 0.04;
  const desiredLength = 65;
  const centerForce = 0.005;
  const damping = 0.88;

  // 1. ノード間斥力 (反発)
  for (let i = 0; i < nodes.length; i++) {{
    const a = nodes[i];
    for (let j = i + 1; j < nodes.length; j++) {{
      const b = nodes[j];
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const distSq = dx * dx + dy * dy + 0.1;
      const dist = Math.sqrt(distSq);
      if (dist < 350) {{
        const force = kRepulse / distSq;
        const fx = (dx / dist) * force;
        const fy = (dy / dist) * force;
        a.vx -= fx;
        a.vy -= fy;
        b.vx += fx;
        b.vy += fy;
      }}
    }}
  }}

  // 2. エッジ引力 (バネ)
  for (const e of edges) {{
    const a = e.sourceNode;
    const b = e.targetNode;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const dist = Math.sqrt(dx * dx + dy * dy) || 1;
    const displacement = dist - desiredLength;
    const force = displacement * kSpring;
    const fx = (dx / dist) * force;
    const fy = (dy / dist) * force;
    a.vx += fx;
    a.vy += fy;
    b.vx -= fx;
    b.vy -= fy;
  }}

  // 3. 中心引力 & 位置更新
  const cx = width / 2;
  const cy = height / 2;
  for (const n of nodes) {{
    if (n === draggedNode) continue;
    n.vx += (cx - n.x) * centerForce;
    n.vy += (cy - n.y) * centerForce;

    n.vx *= damping;
    n.vy *= damping;
    n.x += n.vx;
    n.y += n.vy;
  }}

  // 4. SVG 要素の座標同期
  for (const item of edgeElements) {{
    item.el.setAttribute("x1", item.data.sourceNode.x);
    item.el.setAttribute("y1", item.data.sourceNode.y);
    item.el.setAttribute("x2", item.data.targetNode.x);
    item.el.setAttribute("y2", item.data.targetNode.y);
  }}
  for (const item of nodeElements) {{
    item.el.setAttribute("cx", item.data.x);
    item.el.setAttribute("cy", item.data.y);
  }}
  for (const item of labelElements) {{
    item.el.setAttribute("x", item.data.x);
    item.el.setAttribute("y", item.data.y + item.data.radius + 12);
  }}
}}

// アニメーションループ
function loop() {{
  stepSimulation();
  requestAnimationFrame(loop);
}}
requestAnimationFrame(loop);

// ドラッグ & パン & ズーム操作
let draggedNode = null;
let isPanning = false;
let startX = 0;
let startY = 0;

svg.addEventListener("mousedown", (e) => {{
  if (e.target === svg) {{
    isPanning = true;
    startX = e.clientX - panX;
    startY = e.clientY - panY;
  }}
}});

window.addEventListener("mousemove", (e) => {{
  if (draggedNode) {{
    const rect = svg.getBoundingClientRect();
    draggedNode.x = (e.clientX - rect.left - panX) / zoomScale;
    draggedNode.y = (e.clientY - rect.top - panY) / zoomScale;
    draggedNode.vx = 0;
    draggedNode.vy = 0;
  }} else if (isPanning) {{
    panX = e.clientX - startX;
    panY = e.clientY - startY;
    updateTransform();
  }}
}});

window.addEventListener("mouseup", () => {{
  draggedNode = null;
  isPanning = false;
}});

svg.addEventListener("wheel", (e) => {{
  e.preventDefault();
  const zoomFactor = e.deltaY < 0 ? 1.1 : 0.9;
  zoomScale = Math.max(0.2, Math.min(4.0, zoomScale * zoomFactor));
  updateTransform();
}}, {{ passive: false }});

// ノード詳細選択
function selectNode(node) {{
  const sidebar = document.getElementById('sidebar');
  sidebar.classList.remove('hidden');

  const isMemory = node.id.startsWith("mem_");
  const badge = document.getElementById('detail-category');
  badge.textContent = node.category;
  badge.className = isMemory ? "badge badge-memory" : "badge badge-kg";

  document.getElementById('detail-title').textContent = node.label;
  document.getElementById('detail-body').textContent = node.details;

  const tagsContainer = document.getElementById('detail-tags');
  tagsContainer.innerHTML = '';
  (node.tags || []).forEach(tag => {{
    const t = document.createElement('span');
    t.className = 'badge';
    t.style.background = '#334155';
    t.style.color = '#94a3b8';
    t.textContent = '#' + tag;
    tagsContainer.appendChild(t);
  }});

  // 関連エッジ探索
  const linksList = document.getElementById('detail-links');
  linksList.innerHTML = '';
  const related = edges.filter(e => e.source === node.id || e.target === node.id);
  if (related.length === 0) {{
    linksList.innerHTML = '<li>No connected relations</li>';
  }} else {{
    related.forEach(e => {{
      const other = e.source === node.id ? e.targetNode : e.sourceNode;
      const li = document.createElement('li');
      li.textContent = `${{e.source === node.id ? '─[' + e.relation + ']─►' : '◄─[' + e.relation + ']─'}} ${{other.label}}`;
      linksList.appendChild(li);
    }});
  }}

  // ハイライト
  nodeElements.forEach(item => {{
    if (item.data.id === node.id) {{
      item.el.setAttribute("stroke", "#f59e0b");
      item.el.setAttribute("stroke-width", "3.5");
    }} else {{
      item.el.setAttribute("stroke", "#0f172a");
      item.el.setAttribute("stroke-width", "2");
    }}
  }});
}}

function closeSidebar() {{
  document.getElementById('sidebar').classList.add('hidden');
}}

function resetZoom() {{
  zoomScale = 1.0;
  panX = 0;
  panY = 0;
  updateTransform();
}}

function toggleSimulation() {{
  simRunning = !simRunning;
  document.getElementById('sim-btn').textContent = simRunning ? "Pause Sim" : "Resume Sim";
}}

// 検索ハイライト
document.getElementById('search').addEventListener('input', (e) => {{
  const query = e.target.value.toLowerCase().trim();
  nodeElements.forEach(item => {{
    if (!query) {{
      item.el.style.opacity = "1";
      return;
    }}
    const match = item.data.label.toLowerCase().includes(query) ||
                  item.data.details.toLowerCase().includes(query) ||
                  (item.data.tags || []).some(t => t.toLowerCase().includes(query));
    item.el.style.opacity = match ? "1" : "0.15";
  }});
}});
</script>
</body>
</html>
"##,
        safe_json = safe_json
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::export::GraphNode;

    #[test]
    fn test_generate_standalone_html_structure() {
        let snapshot = GraphSnapshot {
            exported_at: "2026-09-07T12:00:00Z".to_string(),
            total_memories: 1,
            total_entities: 1,
            nodes: vec![
                GraphNode {
                    id: "mem_1".to_string(),
                    label: "Test Memory".to_string(),
                    category: "memory".to_string(),
                    details: "This is a detailed memory test.".to_string(),
                    tags: vec!["test".to_string()],
                    weight: 1.5,
                },
                GraphNode {
                    id: "kg_1".to_string(),
                    label: "Rust Entity".to_string(),
                    category: "language".to_string(),
                    details: "Rust programming language".to_string(),
                    tags: vec!["lang".to_string()],
                    weight: 2.0,
                },
            ],
            edges: vec![],
        };

        let html = generate_standalone_html(&snapshot);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("bonsai-agent Memory &amp; Knowledge Graph"));
        assert!(html.contains("Test Memory"));
        assert!(html.contains("Rust Entity"));
        assert!(html.contains("stepSimulation"));
        assert!(html.contains("<svg id=\"canvas\">"));
    }

    #[test]
    fn test_export_and_render_html_integration() {
        let store = MemoryStore::in_memory().expect("in memory store");
        store
            .save_memory("Integration check for HTML", "fact", &["html".to_string()])
            .expect("save");

        let html = export_and_render_html(&store, true).expect("render");
        assert!(html.contains("Integration check for HTML"));
        assert!(html.contains("\"total_memories\": 1") || html.contains("\"total_memories\":1"));
    }
}
