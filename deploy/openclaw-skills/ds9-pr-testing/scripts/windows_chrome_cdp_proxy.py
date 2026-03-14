#!/usr/bin/env python3
import argparse
import socket
import threading


def pipe(source: socket.socket, dest: socket.socket) -> None:
    try:
        while True:
            data = source.recv(65536)
            if not data:
                return
            dest.sendall(data)
    except OSError as exc:
        print(f"pipe-error: {exc}", flush=True)


def close_quietly(conn: socket.socket) -> None:
    try:
        conn.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    conn.close()


def relay(client: socket.socket, target_host: str, target_port: int) -> None:
    upstream = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        upstream.connect((target_host, target_port))
        print(f"accepted {client.getpeername()} -> {target_host}:{target_port}", flush=True)
        threads = [
            threading.Thread(target=pipe, args=(client, upstream), daemon=True),
            threading.Thread(target=pipe, args=(upstream, client), daemon=True),
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
    except OSError as exc:
        print(f"relay-error: {exc}", flush=True)
    finally:
        close_quietly(client)
        close_quietly(upstream)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen-host", default="0.0.0.0")
    parser.add_argument("--listen-port", type=int, default=9223)
    parser.add_argument("--target-host", default="127.0.0.1")
    parser.add_argument("--target-port", type=int, default=9222)
    args = parser.parse_args()

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind((args.listen_host, args.listen_port))
    server.listen(64)
    print(
        f"listening on {args.listen_host}:{args.listen_port} -> "
        f"{args.target_host}:{args.target_port}",
        flush=True,
    )
    try:
        while True:
            client, _ = server.accept()
            thread = threading.Thread(
                target=relay,
                args=(client, args.target_host, args.target_port),
                daemon=True,
            )
            thread.start()
    finally:
        server.close()


if __name__ == "__main__":
    raise SystemExit(main())
