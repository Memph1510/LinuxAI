"""A local mail sink for the VM checks: SMTP over TLS on 127.0.0.1, accepts any login, keeps each message as a file.
Usage: python3 smtp_sink.py CERT KEY PORT OUTDIR"""
import base64
import socket
import ssl
import sys
import threading
from pathlib import Path

cert, key, port, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), Path(sys.argv[4])
out.mkdir(parents=True, exist_ok=True)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)


def serve(conn):
    try:
        f = conn.makefile("rwb")
        say = lambda line: (f.write(line.encode() + b"\r\n"), f.flush())
        say("220 sink ready")
        rcpt = []
        while True:
            line = f.readline()
            if not line:
                return
            cmd = line.decode(errors="replace").strip()
            up = cmd.upper()
            if up.startswith(("EHLO", "HELO")):
                say("250-sink")
                say("250 AUTH PLAIN LOGIN")
            elif up.startswith("AUTH PLAIN"):
                if up == "AUTH PLAIN":  # credentials on the next line
                    say("334 ")
                    f.readline()
                say("235 ok")
            elif up.startswith("AUTH LOGIN"):
                say("334 " + base64.b64encode(b"Username:").decode())
                f.readline()
                say("334 " + base64.b64encode(b"Password:").decode())
                f.readline()
                say("235 ok")
            elif up.startswith("MAIL FROM"):
                rcpt = []
                say("250 ok")
            elif up.startswith("RCPT TO"):
                rcpt.append(cmd[8:].strip())
                say("250 ok")
            elif up == "DATA":
                say("354 go")
                lines = []
                while True:
                    ln = f.readline()
                    if ln in (b".\r\n", b".\n", b""):
                        break
                    lines.append(ln)
                n = len(list(out.glob("*.eml")))
                (out / f"{n + 1}.eml").write_bytes(b"".join(lines))
                say("250 kept")
            elif up == "QUIT":
                say("221 bye")
                return
            else:
                say("250 ok")
    finally:
        conn.close()


with socket.create_server(("127.0.0.1", port)) as srv:
    while True:
        c, _ = srv.accept()
        try:
            threading.Thread(target=serve, args=(ctx.wrap_socket(c, server_side=True),), daemon=True).start()
        except ssl.SSLError:
            c.close()
