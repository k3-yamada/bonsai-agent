# Ruri v3 embed sidecar（参照実装）

仕様の SSOT: [`docs/execution/ruri-embed-sidecar.md`](../../docs/execution/ruri-embed-sidecar.md)  
決定: [`docs/decisions/ADR-015-ruri-v3-local-embeddings.md`](../../docs/decisions/ADR-015-ruri-v3-local-embeddings.md)

```bash
python3 -m venv .venv-ruri && source .venv-ruri/bin/activate
pip install -r scripts/ruri_embed_server/requirements.txt
./scripts/start-ruri-embed.sh
```

prefix ヘルパのみ検証（モデル不要）:

```bash
python3 - <<'PY'
import importlib.util
from pathlib import Path
p = Path("scripts/ruri_embed_server/server.py")
spec = importlib.util.spec_from_file_location("ruri_server", p)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
assert m.apply_ruri_prefix("頭痛", "query") == "検索クエリ: 頭痛"
assert m.apply_ruri_prefix("検索クエリ: 頭痛", "query") == "検索クエリ: 頭痛"
print("ok")
PY
```
