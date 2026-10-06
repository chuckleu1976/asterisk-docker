"""Drive tg2sip's interactive `python -m src.auth` from the management page.

Pyrogram asks for the phone number, confirms it, then the login code, then a
2FA password when the account has one. Those prompts are answered on a PTY so
getpass (the 2FA prompt) reads the same terminal as input().

One JSON object in, one JSON object out, on a unix socket. The phone number,
code, and password are not written to the log.
"""

from __future__ import annotations

import json
import os
import pty
import re
import select
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RUNTIME = REPO / "session" / "tg2sip-login"

PROMPTS = (
    ("signed_in", "signed in as "),
    ("firstname", "Enter first name:"),
    ("password", "Enter password (empty to recover):"),
    ("code", "Enter confirmation code:"),
    ("confirm", "correct? (y/N):"),
    ("phone", "Enter phone number or bot token:"),
)
ERRORS = (
    "PHONE_NUMBER_INVALID",
    "PHONE_CODE_INVALID",
    "PHONE_CODE_EXPIRED",
    "PASSWORD_HASH_INVALID",
    "FLOOD_WAIT",
    "required env var",
    "API_ID_INVALID",
)


def classify(text: str) -> tuple[str | None, int]:
    """Return the newest prompt or error marker and its position."""
    err_pos = -1
    for marker in ERRORS:
        err_pos = max(err_pos, text.rfind(marker))
    best_kind = None
    best_pos = -1
    for kind, marker in PROMPTS:
        pos = text.rfind(marker)
        if pos > best_pos:
            best_pos = pos
            best_kind = kind
    if err_pos > best_pos and best_kind not in ("code", "password", "signed_in"):
        return "error", err_pos
    return best_kind, best_pos


class Login:
    def __init__(self, instance: int) -> None:
        self.instance = instance
        self.service = f"tg2sip{instance}"
        self.pid: int | None = None
        self.fd: int | None = None
        self.phase = "idle"
        self.error = ""
        self._buf = ""

    def status(self) -> dict[str, str]:
        body = {"session": _session_label(self.phase)}
        if self.error and self.phase in ("code_invalid", "password_invalid"):
            body["error"] = self.error
        return body

    def submit(self, op: str, value: str) -> dict[str, str]:
        self.error = ""
        if op == "phone":
            self._restart()
            kind, new = self._drive(180)
            if kind != "phone":
                redacted = re.sub(r"\d", "0", new.replace("\r", "\n"))
                print(f"login prompt kind={kind} buf={redacted[-800:]}", file=sys.stderr, flush=True)
                return self._fail(_phone_prompt_failure(kind, new))
            self._write(value)
            kind, new = self._drive(90)
        elif op == "code":
            if self.phase not in ("code", "code_invalid"):
                return self._fail("Enter the gateway phone number first")
            self._write(value)
            kind, new = self._drive(60)
        elif op == "password":
            if self.phase not in ("password", "password_invalid"):
                return self._fail("This account is not asking for a 2FA password")
            self._write(value)
            kind, new = self._drive(60)
        else:
            return self._fail("Unknown login step")

        if kind == "signed_in":
            self._wait_exit()
            self.phase = "signed_in"
            self.error = ""
            return self.status()
        if kind == "code":
            if "PHONE_CODE_INVALID" in new or "PHONE_CODE_EXPIRED" in new:
                self.phase = "code_invalid"
                self.error = "Telegram rejected that code"
            else:
                self.phase = "code"
            return self.status()
        if kind == "password":
            if "PASSWORD_HASH_INVALID" in new:
                self.phase = "password_invalid"
                self.error = "Telegram rejected that 2FA password"
            else:
                self.phase = "password"
            return self.status()
        if kind == "firstname":
            return self._fail("That phone number is not a Telegram account yet")
        if kind == "phone":
            return self._fail("Telegram rejected that phone number")
        if kind == "error":
            return self._fail(_error_text(new))
        if kind == "timeout":
            return self._fail("Timed out waiting for Telegram")
        if kind == "eof":
            return self._fail("The login process ended before Telegram accepted it")
        return self._fail("Telegram login stopped")

    def close(self) -> None:
        if self.pid:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except OSError:
                pass
            self.pid = None
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None
        self._remove_run_containers()

    def _remove_run_containers(self) -> None:
        listed = subprocess.run(
            ["docker", "ps", "-aq", "--filter", f"name={self.service}-run"],
            capture_output=True,
            text=True,
            check=False,
        )
        ids = [line for line in listed.stdout.split() if line]
        if ids:
            subprocess.run(["docker", "rm", "-f", *ids], capture_output=True, text=True, check=False)

    def _restart(self) -> None:
        self.close()
        self._remove_run_containers()
        self._buf = ""
        self.phase = "phone"
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(REPO)
            os.execvp(
                "docker",
                [
                    "docker",
                    "compose",
                    "run",
                    "--rm",
                    "--no-deps",
                    self.service,
                    "python",
                    "-m",
                    "src.auth",
                ],
            )
        self.pid = pid
        self.fd = fd

    def _write(self, value: str) -> None:
        if self.fd is None:
            raise RuntimeError("login process is not running")
        os.write(self.fd, (value + "\n").encode())

    def _drive(self, timeout: float) -> tuple[str, str]:
        assert self.fd is not None
        start = len(self._buf)
        deadline = time.time() + timeout
        answered_confirm = -1
        while time.time() < deadline:
            remaining = deadline - time.time()
            ready, _, _ = select.select([self.fd], [], [], min(0.5, remaining))
            if self.pid and not ready:
                waited, status = os.waitpid(self.pid, os.WNOHANG)
                if waited:
                    self.pid = None
                    if "signed in as " in self._buf[start:]:
                        return "signed_in", self._buf[start:]
                    if os.waitstatus_to_exitcode(status) != 0:
                        return "eof", self._buf[start:]
            if ready:
                try:
                    chunk = os.read(self.fd, 4096)
                except OSError:
                    return "eof", self._buf[start:]
                if not chunk:
                    if "signed in as " in self._buf[start:]:
                        return "signed_in", self._buf[start:]
                    return "eof", self._buf[start:]
                self._buf += chunk.decode("utf-8", "replace")
            kind, pos = classify(self._buf)
            if kind is None or pos < start:
                continue
            if kind == "confirm":
                if pos != answered_confirm:
                    os.write(self.fd, b"y\n")
                    answered_confirm = pos
                continue
            return kind, self._buf[start:]
        return "timeout", self._buf[start:]

    def _wait_exit(self) -> None:
        if self.pid:
            try:
                os.waitpid(self.pid, 0)
            except ChildProcessError:
                pass
            self.pid = None
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None

    def _fail(self, message: str) -> dict[str, str]:
        self.phase = "idle"
        self.error = message
        self.close()
        return {"session": "not logged in", "error": message}


def _session_label(phase: str) -> str:
    if phase in ("code", "code_invalid"):
        return "login needs a code"
    if phase in ("password", "password_invalid"):
        return "login needs a password"
    if phase == "signed_in":
        return "logged in"
    return "not logged in"


def _public_tail(text: str) -> str:
    lines = []
    for line in text.replace("\r", "\n").splitlines():
        line = line.strip()
        if not line or any(ch.isdigit() for ch in line):
            continue
        lines.append(line[:160])
    return lines[-1] if lines else ""


def _phone_prompt_failure(kind: str, text: str) -> str:
    if kind == "timeout":
        detail = "Timed out waiting for Telegram to ask for the phone number"
    elif kind == "eof":
        detail = "The login process ended before asking for a phone number"
    elif kind == "error":
        detail = _error_text(text)
    else:
        detail = "Telegram login did not ask for a phone number"
    tail = _public_tail(text)
    if tail and tail not in detail:
        return f"{detail}: {tail}"
    return detail


def _error_text(text: str) -> str:
    for marker in ERRORS:
        if marker in text:
            return marker
    return "Telegram login failed"


def serve(instance: int) -> None:
    RUNTIME.mkdir(parents=True, exist_ok=True)
    path = RUNTIME / f"{instance}.sock"
    if path.exists():
        path.unlink()
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(path))
    os.chmod(path, 0o600)
    server.listen(1)
    login = Login(instance)
    try:
        while True:
            conn, _addr = server.accept()
            try:
                raw = b""
                while b"\n" not in raw and len(raw) < 8192:
                    chunk = conn.recv(4096)
                    if not chunk:
                        break
                    raw += chunk
                if not raw:
                    continue
                req = json.loads(raw.decode())
                op = req.get("op", "status")
                if op == "status":
                    resp: dict[str, str] = {"session": _session_label(login.phase)}
                    if login.error and login.phase in ("code_invalid", "password_invalid"):
                        resp["error"] = login.error
                elif op == "shutdown":
                    try:
                        conn.sendall(b'{"ok": true}\n')
                    except OSError:
                        pass
                    break
                else:
                    resp = login.submit(op, str(req.get("value", "")))
                try:
                    conn.sendall((json.dumps(resp) + "\n").encode())
                except OSError:
                    continue
            except (json.JSONDecodeError, UnicodeError):
                continue
            finally:
                conn.close()
    finally:
        login.close()
        server.close()
        path.unlink(missing_ok=True)


if __name__ == "__main__":
    import sys

    serve(int(sys.argv[1]))
