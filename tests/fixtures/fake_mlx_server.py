#!/usr/bin/env python3
"""B-1 e2e テスト用の最小 fake MLX server。

ProcessSupervisor.build_spawn_args が組む mlx-openai-server 互換の引数列
(`launch --model-path <M> --model-type lm --port <P>`) を受理し、任意 path に
`200 OK` を返すだけの HTTP server。実 MLX/モデル不要で spawn→health→kill→respawn
の実プロセス往復を検証するために使う。
"""
import argparse
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


class _Ok(BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802
        self.send_response(200)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, *args):  # 黙らせる
        pass


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("subcommand", nargs="?", default="launch")  # "launch" を吸収
    p.add_argument("--model-path", default="")
    p.add_argument("--model-type", default="lm")
    p.add_argument("--port", type=int, default=8000)
    args, _unknown = p.parse_known_args()
    server = HTTPServer(("127.0.0.1", args.port), _Ok)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    sys.exit(main())
