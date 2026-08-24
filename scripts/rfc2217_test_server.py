#!/usr/bin/env python3
"""A local RFC 2217 access server for testing AuraTerm's network serial support.

Built on pyserial's own ``PortManager``, so it is the reference implementation
of the server side rather than a second copy of our understanding — a protocol
error in AuraTerm shows up here as a real interop failure.

The "device" behind the port is a pty pair whose far end echoes every byte back,
so no hardware is needed.

    pip install pyserial
    python3 scripts/rfc2217_test_server.py 2217

Then either connect from the app (Serial -> Network (RFC 2217) -> 127.0.0.1:2217)
or run the transport's interop test against it:

    AURATERM_RFC2217_ENDPOINT=127.0.0.1:2217 \
      cargo test --manifest-path src-tauri/Cargo.toml -- --ignored rfc2217_interop

On exit the server prints the parameters it ended up with, which is the direct
check that the client's parameter block actually took effect.
"""
import os
import pty
import socket
import sys
import threading
import time

import serial
import serial.rfc2217


class PtySerial(serial.Serial):
    """A pty has no modem lines: TIOCMGET/TIOCMSET raise ENOTTY on one.

    Stubbing the four inputs and two outputs lets pyserial's PortManager run
    unmodified against a fake device. Nothing the protocol test cares about —
    option 44 framing, the parameter block, IAC escaping — is affected.
    """

    def __init__(self, *args, **kwargs):
        self._fake_dtr = True
        self._fake_rts = True
        super().__init__(*args, **kwargs)

    cts = property(lambda self: False)
    dsr = property(lambda self: False)
    ri = property(lambda self: False)
    cd = property(lambda self: False)

    @property
    def dtr(self):
        return self._fake_dtr

    @dtr.setter
    def dtr(self, value):
        self._fake_dtr = bool(value)

    @property
    def rts(self):
        return self._fake_rts

    @rts.setter
    def rts(self, value):
        self._fake_rts = bool(value)

    def _update_dtr_state(self):
        pass

    def _update_rts_state(self):
        pass


class Connection:
    """The write sink PortManager expects."""

    def __init__(self, sock):
        self.sock = sock

    def write(self, data):
        self.sock.sendall(bytes(data))


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 2217
    timeout = float(sys.argv[2]) if len(sys.argv) > 2 else 300.0

    master_fd, slave_fd = pty.openpty()
    device = os.ttyname(slave_fd)
    print(f"pty device: {device}", flush=True)

    def echo_device():
        while True:
            try:
                data = os.read(master_fd, 1024)
            except OSError:
                return
            if not data:
                return
            os.write(master_fd, data)

    threading.Thread(target=echo_device, daemon=True).start()
    ser = PtySerial(device, baudrate=9600, timeout=0.05)

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", port))
    srv.listen(1)
    print(f"listening on 127.0.0.1:{port} (echoes everything back)", flush=True)

    conn, addr = srv.accept()
    print(f"client connected: {addr}", flush=True)
    conn.settimeout(0.05)
    manager = serial.rfc2217.PortManager(ser, Connection(conn))

    deadline = time.time() + timeout
    try:
        while time.time() < deadline:
            try:
                data = conn.recv(1024)
                if not data:
                    break
                # filter() and escape() are generators of bytes objects, so
                # serial.to_bytes() cannot consume them.
                ser.write(b"".join(manager.filter(data)))
            except socket.timeout:
                pass
            except OSError:
                break

            data = ser.read(64)
            if data:
                conn.sendall(b"".join(manager.escape(data)))
            manager.check_modem_lines()
    finally:
        print(
            f"FINAL baudrate={ser.baudrate} bytesize={ser.bytesize} "
            f"parity={ser.parity} stopbits={ser.stopbits} "
            f"rtscts={ser.rtscts} xonxoff={ser.xonxoff} "
            f"dtr={ser.dtr} rts={ser.rts}",
            flush=True,
        )
        conn.close()
        srv.close()


if __name__ == "__main__":
    main()
