# Third-Party Licenses

bonsai-agent は以下の OSS を逐語 port または依存しています。各ライセンスを尊重し、
バイナリ配布または source 配布時には本ファイルを同梱してください。

---

## cerememory (MIT)

- **Source**: https://github.com/co-r-e/cerememory
- **Used commit**: `b08d201` (2026-05-08)
- **License**: MIT
- **Copyright**: 2026 CORe Inc.
- **Used in bonsai**:
  - `src/memory/decay.rs` — `crates/cerememory-decay/src/math.rs` から 4 純関数を逐語 port
    (`compute_fidelity` / `compute_noise` / `compute_stability_boost` / `compute_emotion_mod`)
  - 関連: 項目 217 (Cerememory power-law decay port)
  - Plan: [`.claude/plan/cerememory-decay-port-impl.md`](../.claude/plan/cerememory-decay-port-impl.md)

### MIT License (full text)

```
MIT License

Copyright (c) 2026 CORe Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
