"""yote CLI — Python wrapper around the Rust yote binary + streaming commands."""

import argparse
import os
import signal
import socket
import struct
import subprocess
import sys
import threading
import zlib

import yote

MAGIC_HEADER = b"YOTE\x01"
MAGIC_FOOTER = b"YOTE\x00"
CHUNK_SIZE = 960  # bytes per chunk for encode (Opus frame size friendly)


# ---------------------------------------------------------------------------
# Protocol helpers
# ---------------------------------------------------------------------------

def _make_payload(data: bytes, filename: str = "received.bin") -> bytes:
    """Compress data and prepend header: magic + filename_len + filename + data_len + crc32."""
    compressed = zlib.compress(data)
    crc = zlib.crc32(data) & 0xFFFFFFFF
    fname_bytes = filename.encode("utf-8")
    header = (
        MAGIC_HEADER
        + struct.pack("<H", len(fname_bytes))
        + fname_bytes
        + struct.pack("<I", len(compressed))
        + struct.pack("<I", crc)
    )
    return header + compressed


def _parse_payload(payload: bytes):
    """Parse header, decompress, verify CRC. Returns (filename, original_data)."""
    if not payload.startswith(MAGIC_HEADER):
        raise ValueError("Bad magic header")
    off = len(MAGIC_HEADER)
    fname_len = struct.unpack("<H", payload[off:off + 2])[0]
    off += 2
    filename = payload[off:off + fname_len].decode("utf-8")
    off += fname_len
    data_len = struct.unpack("<I", payload[off:off + 4])[0]
    off += 4
    crc_expected = struct.unpack("<I", payload[off:off + 4])[0]
    off += 4
    compressed = payload[off:off + data_len]
    if len(compressed) != data_len:
        raise ValueError(f"Expected {data_len} bytes of compressed data, got {len(compressed)}")
    data = zlib.decompress(compressed)
    crc_actual = zlib.crc32(data) & 0xFFFFFFFF
    if crc_actual != crc_expected:
        raise ValueError(f"CRC mismatch: expected {crc_expected:#x}, got {crc_actual:#x}")
    return filename, data


def _encode_and_frame(data: bytes, bitrate: int = 128) -> bytes:
    """Encode data into Opus packets, framed per-chunk, with footer.

    Wire format per chunk:
        2-byte LE packet_count  (number of packets in this chunk)
        For each packet: 2-byte LE length + packet_bytes
    After all chunks:
        2-byte LE 0 (zero packet_count = end marker) + MAGIC_FOOTER
    """
    chunks = [data[i:i + CHUNK_SIZE] for i in range(0, len(data), CHUNK_SIZE)]
    # Pad last chunk to CHUNK_SIZE
    if chunks and len(chunks[-1]) < CHUNK_SIZE:
        chunks[-1] = chunks[-1] + b"\x00" * (CHUNK_SIZE - len(chunks[-1]))

    buf = bytearray()
    total_packets = 0
    for chunk in chunks:
        pkt_list = [bytes(p) for p in yote.encode(chunk, bitrate=bitrate)]
        buf += struct.pack("<H", len(pkt_list))
        for pkt in pkt_list:
            buf += struct.pack("<H", len(pkt)) + pkt
        total_packets += len(pkt_list)

    # End marker + footer magic
    buf += struct.pack("<H", 0) + MAGIC_FOOTER
    return bytes(buf), total_packets


def _read_and_decode_chunks(read_fn) -> bytes:
    """Read framed chunks, decode each chunk's packets, return reassembled bytes."""
    result = bytearray()
    total_packets = 0
    while True:
        # Read chunk header: packet count
        hdr = read_fn(2)
        if not hdr or len(hdr) < 2:
            raise ValueError("Unexpected end of stream reading chunk header")
        pkt_count = struct.unpack("<H", hdr)[0]
        if pkt_count == 0:
            # End marker — read and verify footer
            footer = read_fn(len(MAGIC_FOOTER))
            if footer != MAGIC_FOOTER:
                raise ValueError(f"Expected footer magic, got {footer!r}")
            break
        # Read this chunk's packets
        packets = []
        for _ in range(pkt_count):
            len_buf = read_fn(2)
            if not len_buf or len(len_buf) < 2:
                raise ValueError("Unexpected end of stream reading packet length")
            pkt_len = struct.unpack("<H", len_buf)[0]
            pkt = bytearray()
            while len(pkt) < pkt_len:
                chunk = read_fn(pkt_len - len(pkt))
                if not chunk:
                    raise ValueError("Unexpected end of stream reading packet data")
                pkt.extend(chunk)
            packets.append(bytes(pkt))
        # Decode this chunk
        decoded = yote.decode(packets)
        result.extend(decoded)
        total_packets += pkt_count
    return bytes(result), total_packets


# ---------------------------------------------------------------------------
# Transport helpers
# ---------------------------------------------------------------------------

def _parse_via(via_str: str):
    """Parse --via argument. Returns (type, params) tuple."""
    if via_str is None or via_str == "pipe":
        return ("pipe", {})
    if via_str.startswith("port:"):
        parts = via_str[5:].split(":")
        if len(parts) == 2:
            return ("port", {"host": parts[0], "port": int(parts[1])})
        elif len(parts) == 1:
            return ("port", {"host": "localhost", "port": int(parts[0])})
        else:
            raise ValueError(f"Invalid port spec: {via_str}")
    raise ValueError(f"Unknown transport: {via_str}")


def _recv_exact(sock, n):
    """Read exactly n bytes from socket."""
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            if len(buf) == 0:
                return b""
            raise ConnectionError("Connection closed mid-read")
        buf.extend(chunk)
    return bytes(buf)


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

def cmd_yip(args):
    out = yote.yip(args.path, bitrate=args.bitrate, depth=args.depth)
    print(f"-> {out}")


def _yawp_status(args):
    if getattr(args, "no_yawp", False):
        return "(FFT-only decoding)"
    if yote.HAS_YAWP:
        return "(yawp neural correction active)"
    return "(FFT-only decoding)"


def cmd_unyip(args):
    print(_yawp_status(args), file=sys.stderr)
    out = yote.unyip(args.path)
    print(f"-> {out}")


def cmd_info(args):
    meta = yote.info(args.path)
    for k, v in meta.items():
        print(f"  {k}: {v}")


def cmd_stats(args):
    s = yote.stats()
    for name, info in s.items():
        print(f"{name}:")
        for k, v in info.items():
            print(f"  {k}: {v}")
        print()


def cmd_tx(args):
    # Read input
    if args.file == "--stdin" or args.file == "-":
        data = sys.stdin.buffer.read()
        filename = "received.bin"
    else:
        with open(args.file, "rb") as f:
            data = f.read()
        filename = os.path.basename(args.file)

    # Build payload (compress + header)
    payload = _make_payload(data, filename)

    # Encode to Opus and frame
    framed, num_packets = _encode_and_frame(payload, bitrate=getattr(args, "bitrate", 128))

    transport_type, params = _parse_via(args.via)
    print(f"Sending {len(data)} bytes ({len(payload)} compressed) in {num_packets} packets...",
          file=sys.stderr)

    if transport_type == "pipe":
        sys.stdout.buffer.write(framed)
        sys.stdout.buffer.flush()
    elif transport_type == "port":
        host = params["host"]
        port = params["port"]
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(10)
            sock.connect((host, port))
            sock.sendall(framed)
            sock.shutdown(socket.SHUT_WR)
            sock.close()
            print(f"Sent to {host}:{port}", file=sys.stderr)
        except (ConnectionRefusedError, socket.timeout) as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)


def cmd_rx(args):
    transport_type, params = _parse_via(args.via)
    output_dir = args.output or "."

    if transport_type == "pipe":
        read_fn = lambda n: sys.stdin.buffer.read(n)
    elif transport_type == "port":
        port = params["port"]
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("0.0.0.0", port))
        srv.listen(1)
        print(f"Listening on port {port}...", file=sys.stderr)
        conn, addr = srv.accept()
        print(f"Connection from {addr}", file=sys.stderr)
        read_fn = lambda n: _recv_exact(conn, n)
    else:
        print(f"Unknown transport", file=sys.stderr)
        sys.exit(1)

    print(_yawp_status(args), file=sys.stderr)
    try:
        raw, num_packets = _read_and_decode_chunks(read_fn)
        print(f"Received {num_packets} packets", file=sys.stderr)

        # Parse payload from decoded data (strips padding via header length)
        filename, data = _parse_payload(raw)

        os.makedirs(output_dir, exist_ok=True)
        outpath = os.path.join(output_dir, filename)
        with open(outpath, "wb") as f:
            f.write(data)
        print(f"Received {len(data)} bytes -> {outpath}", file=sys.stderr)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        if transport_type == "port":
            conn.close()
            srv.close()


def cmd_link(args):
    transport_type, params = _parse_via(args.via)

    if transport_type == "pipe":
        print("Error: pipe transport is unidirectional, cannot use with link", file=sys.stderr)
        sys.exit(1)

    if transport_type != "port":
        print(f"Error: unsupported transport for link", file=sys.stderr)
        sys.exit(1)

    port = params["port"]
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("0.0.0.0", port))
    srv.listen(1)
    print(f"yote link listening on port {port}... waiting for connection", file=sys.stderr)
    conn, addr = srv.accept()
    print(f"Connected: {addr}", file=sys.stderr)

    running = threading.Event()
    running.set()

    def reader_thread():
        """Read and decode incoming messages."""
        try:
            while running.is_set():
                try:
                    raw, _ = _read_and_decode_chunks(lambda n: _recv_exact(conn, n))
                    filename, data = _parse_payload(raw)
                    print(f"\n< {data.decode('utf-8', errors='replace')}")
                    print("> ", end="", flush=True)
                except (ConnectionError, ValueError):
                    break
        except Exception:
            pass
        finally:
            running.clear()

    t = threading.Thread(target=reader_thread, daemon=True)
    t.start()

    print("> ", end="", flush=True)
    try:
        while running.is_set():
            line = sys.stdin.readline()
            if not line:
                break
            msg = line.rstrip("\n").encode("utf-8")
            if not msg:
                print("> ", end="", flush=True)
                continue
            payload = _make_payload(msg, "msg")
            framed, _ = _encode_and_frame(payload)
            try:
                conn.sendall(framed)
            except BrokenPipeError:
                print("Connection closed", file=sys.stderr)
                break
            print("> ", end="", flush=True)
    except KeyboardInterrupt:
        pass
    finally:
        running.clear()
        conn.close()
        srv.close()
        print("\nDisconnected.", file=sys.stderr)


def cmd_install(args):
    if args.stop:
        pidfile = os.path.expanduser("~/.yote/daemon.pid")
        if not os.path.exists(pidfile):
            print("No daemon running (no PID file found)", file=sys.stderr)
            sys.exit(1)
        with open(pidfile) as f:
            pid = int(f.read().strip())
        try:
            os.kill(pid, signal.SIGTERM)
            print(f"yote daemon (PID {pid}) stopped", file=sys.stderr)
        except ProcessLookupError:
            print(f"Daemon PID {pid} not running", file=sys.stderr)
        os.remove(pidfile)
        return

    port = args.port
    if port is None:
        print("Error: -p PORT is required", file=sys.stderr)
        sys.exit(1)

    recv_dir = os.path.expanduser("~/yote-received")
    os.makedirs(recv_dir, exist_ok=True)
    os.makedirs(os.path.expanduser("~/.yote"), exist_ok=True)

    proc = subprocess.Popen(
        [sys.executable, "-m", "yote", "rx", "--via", f"port:{port}", "-o", recv_dir],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )

    pidfile = os.path.expanduser("~/.yote/daemon.pid")
    with open(pidfile, "w") as f:
        f.write(str(proc.pid))

    print(f"yote daemon started on port {port} (PID {proc.pid})")


def main():
    parser = argparse.ArgumentParser(
        prog="yote",
        description="Data-over-audio codec. Smuggle data through Opus audio channels.",
    )
    sub = parser.add_subparsers(dest="command")

    # yip
    p_yip = sub.add_parser("yip", help="Pack file into <file>.yip")
    p_yip.add_argument("path", help="File to pack")
    p_yip.add_argument("--bitrate", type=int, default=128, help="Opus bitrate in kbps")
    p_yip.add_argument("--depth", default="quad", choices=["binary", "quad", "hex16"])
    p_yip.set_defaults(func=cmd_yip)

    # unyip
    p_unyip = sub.add_parser("unyip", help="Unpack .yip back to original file")
    p_unyip.add_argument("path", help=".yip file to unpack")
    p_unyip.add_argument("--no-yawp", action="store_true", help="Disable neural correction")
    p_unyip.set_defaults(func=cmd_unyip)

    # info
    p_info = sub.add_parser("info", help="Show .yip file metadata")
    p_info.add_argument("path", help=".yip file to inspect")
    p_info.set_defaults(func=cmd_info)

    # stats
    p_stats = sub.add_parser("stats", help="Show throughput and capacity stats")
    p_stats.set_defaults(func=cmd_stats)

    # tx
    p_tx = sub.add_parser("tx", help="Transmit data as Opus stream")
    p_tx.add_argument("file", nargs="?", default="--stdin", help="File to send (default: stdin)")
    p_tx.add_argument("--via", default="pipe", help="Transport: pipe | port:HOST:PORT | port:PORT")
    p_tx.add_argument("--bitrate", type=int, default=128, help="Opus bitrate in kbps")
    p_tx.set_defaults(func=cmd_tx)

    # rx
    p_rx = sub.add_parser("rx", help="Receive data from Opus stream")
    p_rx.add_argument("--via", default="pipe", help="Transport: pipe | port:PORT")
    p_rx.add_argument("-o", "--output", default=".", help="Output directory")
    p_rx.add_argument("--no-yawp", action="store_true", help="Disable neural correction")
    p_rx.set_defaults(func=cmd_rx)

    # link
    p_link = sub.add_parser("link", help="Bidirectional audio link")
    p_link.add_argument("--via", required=True, help="Transport: port:PORT")
    p_link.set_defaults(func=cmd_link)

    # install
    p_install = sub.add_parser("install", help="Start/stop yote daemon")
    p_install.add_argument("-p", "--port", type=int, help="Port for daemon to listen on")
    p_install.add_argument("--stop", action="store_true", help="Stop running daemon")
    p_install.set_defaults(func=cmd_install)

    args = parser.parse_args()
    if not args.command:
        parser.print_help()
        sys.exit(1)

    args.func(args)


if __name__ == "__main__":
    main()
