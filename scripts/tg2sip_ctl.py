"""Control the two tg2sip gateways used by Asterisk 1 and Asterisk 2.

Prints one JSON object to stdout. Secrets (api hash, SIP password, login
code) are never included in that object.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import socket
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TG2SIP_ROOT = Path(os.environ.get("TG2SIP_ROOT", REPO.parent / "tg2sip"))
RUNTIME = REPO / "session" / "tg2sip-login"

LINES = {
    1: {
        "service": "asterisk",
        "gateway": "tg2sip1",
        "hostname": "asterisk1",
        "domain": "asterisk",
    },
    2: {
        "service": "asterisk2",
        "gateway": "tg2sip2",
        "hostname": "asterisk2",
        "domain": "asterisk2",
    },
}

USER_ID_RE = re.compile(r"^\d{1,15}$")
E164_RE = re.compile(r"^\+[1-9]\d{6,14}$")
USERNAME_RE = re.compile(r"^@[A-Za-z][A-Za-z0-9_]{3,31}$")
DIAL_RE = re.compile(r"^same => n,Dial\(PJSIP/[^,\n]+,60,g\)$", re.M)
CONTEXT_RE = re.compile(r"^\[(?P<name>[^\]]+)\][\s\S]*?(?=^\[|\Z)", re.M)


class CtlError(Exception):
    def __init__(self, message: str, code: int = 1) -> None:
        super().__init__(message)
        self.code = code


def pjsip_tg2sip_block(password: str) -> str:
    return (
        ";===============TG2SIP\n"
        "\n"
        "[tg2sip](endpoint-basic-sip)\n"
        "transport=transport-udp-sip\n"
        "context=from-tg2sip\n"
        "disallow=all\n"
        "allow=ulaw,alaw\n"
        "media_encryption=no\n"
        "auth=tg2sip-auth\n"
        "aors=tg2sip\n"
        "direct_media=no\n"
        "rtp_symmetric=yes\n"
        "force_rport=yes\n"
        "rewrite_contact=yes\n"
        "\n"
        "[tg2sip-auth](auth-userpass-sip)\n"
        "username=tg2sip\n"
        f"password={password}\n"
        "\n"
        "[tg2sip](aor-normal-sip)\n"
        "max_contacts=1\n"
        "remove_existing=yes\n"
        "qualify_frequency=60\n"
        "\n"
    )


def compose_service_lines(instance: int) -> list[str]:
    spec = LINES[instance]
    gateway = spec["gateway"]
    return [
        f"  {gateway}:",
        "    build: ../tg2sip",
        f"    env_file: ../tg2sip/{gateway}.env",
        "    environment:",
        "      - PYTHONUNBUFFERED=1",
        "      - CONFIG_PATH=/app/config/config.yaml",
        "      - SESSION_DIR=/app/sessions",
        "      - SIP_BIND_ADDRESS=0.0.0.0",
        "    command: [python, /app/wait_for_session.py]",
        "    volumes:",
        f"      - ../tg2sip/config{instance}:/app/config:ro",
        f"      - ../tg2sip/sessions{instance}:/app/sessions",
        "      - ../tg2sip/wait_for_session.py:/app/wait_for_session.py:ro",
        "    restart: unless-stopped",
        f"    depends_on: [{spec['service']}]",
    ]


def gateway_password(instance: int) -> str:
    """SIP password shared by Asterisk and that instance's gateway env file."""
    if instance not in LINES:
        return secrets.token_urlsafe(18)
    return ensure_instance(instance)["SIP_PASSWORD"]


def ensure_instance(instance: int) -> dict[str, str]:
    spec = LINES[instance]
    root = TG2SIP_ROOT
    config_dir = root / f"config{instance}"
    session_dir = root / f"sessions{instance}"
    config_dir.mkdir(parents=True, exist_ok=True)
    session_dir.mkdir(parents=True, exist_ok=True)
    os.chmod(session_dir, 0o700)
    yaml_path = config_dir / "config.yaml"
    if not yaml_path.exists():
        example = root / "config.example.yaml"
        if not example.exists():
            raise CtlError(f"tg2sip checkout is missing {example}")
        text = example.read_text()
        text = text.replace("local_port: 5062", "local_port: 5060", 1)
        yaml_path.write_text(text)
    env_path = root / f"{spec['gateway']}.env"
    env = _read_env(env_path) if env_path.exists() else {}
    dirty = not env_path.exists()
    if not env.get("SIP_PASSWORD"):
        env["SIP_PASSWORD"] = secrets.token_urlsafe(18)
        dirty = True
    defaults = {
        "TG_API_ID": "",
        "TG_API_HASH": "",
        "TG_FORWARD_USER_ID": "",
        "TG_FORWARD_TARGET": "",
        "SIP_USERNAME": "tg2sip",
        "SIP_DOMAIN": spec["domain"],
        "SIP_REGISTRAR": f"{spec['domain']}:5060",
        "SIP_BIND_ADDRESS": "0.0.0.0",
        "VIDEO_SOURCE_URL": "",
        "LOG_LEVEL": "INFO",
    }
    for key, value in defaults.items():
        if key not in env:
            env[key] = value
            dirty = True
    if dirty:
        _write_env(env_path, env)
    return env


def _read_env(path: Path) -> dict[str, str]:
    data: dict[str, str] = {}
    if not path.exists():
        return data
    for line in path.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        data[key] = value
    return data


def _write_env(path: Path, data: dict[str, str]) -> None:
    order = [
        "TG_API_ID",
        "TG_API_HASH",
        "TG_FORWARD_USER_ID",
        "TG_FORWARD_TARGET",
        "SIP_USERNAME",
        "SIP_PASSWORD",
        "SIP_DOMAIN",
        "SIP_REGISTRAR",
        "SIP_BIND_ADDRESS",
        "VIDEO_SOURCE_URL",
        "LOG_LEVEL",
    ]
    lines = [f"{key}={data.get(key, '')}" for key in order]
    for key, value in data.items():
        if key not in order:
            lines.append(f"{key}={value}")
    path.write_text("\n".join(lines) + "\n")
    os.chmod(path, 0o600)


def _upsert_env(path: Path, updates: dict[str, str]) -> None:
    data = _read_env(path)
    data.update(updates)
    _write_env(path, data)


def _line(instance: int) -> dict[str, str]:
    try:
        return LINES[int(instance)]
    except (KeyError, TypeError, ValueError) as exc:
        raise CtlError("instance must be 1 or 2") from exc


def _valid_target(value: str) -> bool:
    return bool(USER_ID_RE.match(value) or E164_RE.match(value) or USERNAME_RE.match(value))


def _msisdns() -> dict[int, str]:
    path = REPO / "sms-gateway" / "config.toml"
    if not path.exists():
        return {}
    found: dict[int, str] = {}
    for block in path.read_text().split("[[devices]]")[1:]:
        inst = re.search(r"instance\s*=\s*(\d+)", block)
        ms = re.search(r'msisdn\s*=\s*"([^"]*)"', block)
        if inst and ms:
            found[int(inst.group(1))] = ms.group(1)
    return found


def _docker(args: list[str], timeout: float = 30) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["docker", "compose", *args],
        cwd=REPO,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )


def _service_states() -> dict[str, str]:
    try:
        result = _docker(["ps", "--all", "--format", "{{.Service}} {{.Name}} {{.State}}"])
    except (OSError, subprocess.TimeoutExpired):
        return {}
    states: dict[str, str] = {}
    if result.returncode != 0:
        return states
    for line in result.stdout.splitlines():
        parts = line.split(None, 2)
        if len(parts) != 3:
            continue
        service, name, state = parts
        if "-run-" in name:
            continue
        states[service] = state
    return states


def _remove_login_runs(instance: int) -> None:
    """Drop one-off login containers. A live one holds the session file."""
    gateway = _line(instance)["gateway"]
    listed = subprocess.run(
        ["docker", "ps", "-aq", "--filter", f"name={gateway}-run"],
        capture_output=True,
        text=True,
        check=False,
    )
    ids = [line for line in listed.stdout.split() if line]
    if not ids:
        return
    subprocess.run(["docker", "rm", "-f", *ids], capture_output=True, text=True, check=False)


def _asterisk_rx(service: str, command: str) -> str:
    try:
        result = _docker(["exec", "-T", service, "asterisk", "-rx", command], timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    if result.returncode != 0:
        return ""
    return result.stdout


def _sip_status(service: str) -> str:
    text = _asterisk_rx(service, "pjsip show endpoint tg2sip")
    if not text or "Unable to find object" in text or "No such endpoint" in text:
        return "down"
    if re.search(r"\b(Avail|Reachable|NonQual)\b", text):
        return "registered"
    return "down"


def _call_status(service: str) -> str:
    text = _asterisk_rx(service, "core show channels concise")
    for line in text.splitlines():
        if "tg2sip" in line or "from-tg2sip" in line:
            return "busy"
    endpoint = _asterisk_rx(service, "pjsip show endpoint tg2sip")
    for line in endpoint.splitlines():
        match = re.match(r" +Channel: +(\S+)", line)
        if match and not match.group(1).startswith("<"):
            return "busy"
    return "idle"


def _session_file(instance: int) -> Path:
    return TG2SIP_ROOT / f"sessions{instance}" / "gateway.session"


def _session_state(instance: int) -> str:
    """A Pyrogram file exists before login. Only a stored user id means signed in."""
    path = _session_file(instance)
    if not path.exists():
        return "not logged in"
    try:
        con = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=1)
        try:
            row = con.execute("SELECT user_id FROM sessions").fetchone()
        finally:
            con.close()
    except sqlite3.Error:
        return "not logged in"
    if row and row[0]:
        return "logged in"
    return "not logged in"


def _login_request(instance: int, payload: dict[str, str], timeout: float) -> dict[str, str]:
    path = RUNTIME / f"{instance}.sock"
    if not _login_alive(path):
        _start_login_server(instance)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    sock.connect(str(path))
    try:
        sock.sendall((json.dumps(payload) + "\n").encode())
        raw = b""
        while b"\n" not in raw:
            chunk = sock.recv(4096)
            if not chunk:
                break
            raw += chunk
    finally:
        sock.close()
    if not raw:
        raise CtlError("login helper returned nothing")
    return json.loads(raw.decode())


def _login_alive(path: Path) -> bool:
    if not path.exists():
        return False
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(1)
    try:
        sock.connect(str(path))
        sock.sendall(b'{"op":"status"}\n')
        sock.recv(256)
        return True
    except OSError:
        return False
    finally:
        sock.close()


def _start_login_server(instance: int) -> None:
    RUNTIME.mkdir(parents=True, exist_ok=True)
    log_path = RUNTIME / f"{instance}.log"
    with log_path.open("ab") as log:
        subprocess.Popen(
            [sys.executable, str(REPO / "scripts" / "tg2sip_login.py"), str(instance)],
            cwd=REPO,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    path = RUNTIME / f"{instance}.sock"
    deadline = time.time() + 5
    while time.time() < deadline:
        if _login_alive(path):
            return
        time.sleep(0.1)
    raise CtlError("login helper did not start")


def _forward_target(env: dict[str, str]) -> str:
    target = env.get("TG_FORWARD_TARGET", "").strip()
    if target:
        return target
    user_id = env.get("TG_FORWARD_USER_ID", "").strip()
    if user_id and user_id != "0":
        return user_id
    return ""


def _read_routes(instance: int) -> list[dict[str, str]]:
    path = TG2SIP_ROOT / f"config{instance}" / "config.yaml"
    if not path.exists():
        return []
    text = path.read_text()
    match = re.search(r"^  inbound_routes:(?: \{\})?(?:\n    .*)*", text, re.M)
    if not match or match.group(0).rstrip().endswith("{}"):
        return []
    routes = []
    for line in match.group(0).splitlines()[1:]:
        item = line.strip()
        if not item or item.startswith("#") or ":" not in item:
            continue
        caller, _, dest = item.partition(":")
        routes.append(
            {
                "caller": _unquote(caller.strip()),
                "destination": _unquote(dest.strip()),
            }
        )
    return routes


def _unquote(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        try:
            return json.loads(value) if value[0] == '"' else value[1:-1]
        except json.JSONDecodeError:
            return value[1:-1]
    return value


def _write_routes(instance: int, routes: list[dict[str, str]]) -> None:
    path = TG2SIP_ROOT / f"config{instance}" / "config.yaml"
    text = path.read_text()
    if routes:
        lines = ["  inbound_routes:"]
        for route in routes:
            lines.append(
                f"    {json.dumps(route['caller'])}: {json.dumps(route['destination'])}"
            )
        block = "\n".join(lines)
    else:
        block = "  inbound_routes: {}"
    updated, count = re.subn(
        r"^  inbound_routes:(?: \{\})?(?:\n    .*)*",
        block,
        text,
        count=1,
        flags=re.M,
    )
    if count != 1:
        raise CtlError("config.yaml has no inbound_routes entry")
    path.write_text(updated)


def _set_dial(instance: int, target: str) -> None:
    path = REPO / "config" / str(instance) / "asterisk" / "extensions.conf"
    text = path.read_text()
    if target and not target.isdigit():
        dial = f"same => n,Dial(PJSIP/{target}@tg2sip,60,g)"
    else:
        dial = "same => n,Dial(PJSIP/tg2sip,60,g)"
    updated, count = DIAL_RE.subn(dial, text, count=1)
    if count != 1:
        raise CtlError("dialplan has no tg2sip Dial()")
    path.write_text(updated)
    service = LINES[instance]["service"]
    _docker(["exec", "-T", service, "asterisk", "-rx", "dialplan reload"])


def _require_idle(instance: int) -> None:
    if _call_status(LINES[instance]["service"]) == "busy":
        raise CtlError("gateway is on a call", 409)


def _restart_gateway(instance: int) -> None:
    gateway = LINES[instance]["gateway"]
    state = _service_states().get(gateway, "")
    if not state.startswith("running"):
        return
    result = _docker(["restart", gateway], timeout=90)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise CtlError(detail or f"failed to restart {gateway}")


def status() -> dict[str, object]:
    numbers = _msisdns()
    states = _service_states()
    lines = []
    for instance, spec in LINES.items():
        try:
            env = ensure_instance(instance)
        except CtlError as exc:
            env = {}
            forward = ""
            routes: list[dict[str, str]] = []
            setup_error = str(exc)
        else:
            forward = _forward_target(env)
            routes = _read_routes(instance)
            setup_error = ""
        session = _session_state(instance)
        sock = RUNTIME / f"{instance}.sock"
        if _login_alive(sock):
            try:
                live = _login_request(instance, {"op": "status"}, 2)
            except (OSError, json.JSONDecodeError, CtlError):
                live = {}
            if live.get("session") and live["session"] != "not logged in":
                session = live["session"]
        state = states.get(spec["gateway"], "")
        running = state.startswith("running")
        row: dict[str, object] = {
            "instance": instance,
            "hostname": spec["hostname"],
            "msisdn": numbers.get(instance, ""),
            "container": spec["gateway"],
            "session": session,
            "sip": _sip_status(spec["service"]),
            "call": _call_status(spec["service"]),
            "forward": forward,
            "routes": routes,
            "running": running,
        }
        if setup_error:
            row["error"] = setup_error
        lines.append(row)
    return {"lines": lines}


def set_forward(instance: int, target: str) -> dict[str, object]:
    spec = _line(instance)
    target = target.strip()
    if target and not _valid_target(target):
        raise CtlError("forward target must be a user id, @username, or +E.164 number")
    _require_idle(instance)
    ensure_instance(instance)
    user_id = target if target.isdigit() else ""
    _upsert_env(
        TG2SIP_ROOT / f"{spec['gateway']}.env",
        {"TG_FORWARD_TARGET": target, "TG_FORWARD_USER_ID": user_id},
    )
    _set_dial(instance, target)
    _restart_gateway(instance)
    return {"instance": instance, "forward": target}


def set_routes(instance: int, routes: list[dict[str, str]]) -> dict[str, object]:
    _line(instance)
    cleaned = []
    for route in routes:
        caller = str(route.get("caller", "")).strip()
        destination = str(route.get("destination", "")).strip()
        if not _valid_target(caller):
            raise CtlError(f"caller {caller!r} must be a user id, @username, or +E.164")
        if not E164_RE.match(destination):
            raise CtlError(f"destination {destination!r} must be an E.164 number")
        cleaned.append({"caller": caller, "destination": destination})
    _require_idle(instance)
    ensure_instance(instance)
    _write_routes(instance, cleaned)
    _restart_gateway(instance)
    return {"instance": instance, "routes": cleaned}


def set_power(instance: int, action: str) -> dict[str, object]:
    spec = _line(instance)
    if action not in ("start", "stop"):
        raise CtlError("action must be start or stop")
    gateway = spec["gateway"]
    if action == "stop":
        _require_idle(instance)
        result = _docker(["stop", gateway], timeout=60)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip()
            raise CtlError(detail or f"failed to stop {gateway}")
        return {"instance": instance, "running": False}
    env = ensure_instance(instance)
    if not env.get("TG_API_ID") or not env.get("TG_API_HASH"):
        raise CtlError(
            f"Set TG_API_ID and TG_API_HASH in {TG2SIP_ROOT / (gateway + '.env')} before starting"
        )
    RUNTIME.mkdir(parents=True, exist_ok=True)
    log_path = RUNTIME / f"{gateway}-up.log"
    with log_path.open("ab") as log:
        subprocess.Popen(
            ["docker", "compose", "up", "-d", "--no-deps", gateway],
            cwd=REPO,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    return {"instance": instance, "running": True, "starting": True}


def submit_session(instance: int, step: str, value: str) -> dict[str, str]:
    spec = _line(instance)
    if step not in ("phone", "code", "password"):
        raise CtlError("session step must be phone, code, or password")
    if step != "password" and not value.strip():
        raise CtlError(f"{step} is required")
    if step == "password" and not value:
        raise CtlError("2FA password is required")
    env = ensure_instance(instance)
    if not env.get("TG_API_ID") or not env.get("TG_API_HASH"):
        raise CtlError(
            f"Set TG_API_ID and TG_API_HASH in {spec['gateway']}.env before logging in"
        )
    if step == "phone":
        _remove_login_runs(instance)
        _stop_for_login(instance)
    return _login_request(instance, {"op": step, "value": value}, 200)


def _stop_for_login(instance: int) -> None:
    """Stop the gateway even while Docker is restarting it after a crashed login."""
    gateway = _line(instance)["gateway"]
    state = _service_states().get(gateway, "")
    if not state or state.startswith("exit") or state == "dead":
        return
    _require_idle(instance)
    result = _docker(["stop", gateway], timeout=60)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise CtlError(detail or f"failed to stop {gateway}")
    deadline = time.time() + 20
    while time.time() < deadline:
        state = _service_states().get(gateway, "")
        if not state or state.startswith("exit") or state == "dead":
            return
        time.sleep(0.4)
    raise CtlError(f"{gateway} did not stop")


def _extract_context(text: str, name: str) -> str:
    for match in CONTEXT_RE.finditer(text):
        if match.group("name") == name:
            return match.group(0).rstrip() + "\n"
    raise CtlError(f"example dialplan has no [{name}]")


def _replace_context(text: str, name: str, block: str) -> str:
    pattern = re.compile(rf"^\[{re.escape(name)}\][\s\S]*?(?=^\[|\Z)", re.M)
    if pattern.search(text):
        return pattern.sub(block.rstrip() + "\n\n", text, count=1)
    return text.rstrip() + "\n\n" + block


def sync_live_configs() -> dict[str, object]:
    """Copy the example dialplan and tg2sip endpoint onto instances 1 and 2."""
    example = (REPO / "config" / "example" / "asterisk" / "extensions.conf").read_text()
    volte = _extract_context(example, "volte_ims")
    outbound = _extract_context(example, "from-tg2sip")
    changed_dialplan = []
    changed_pjsip = []
    for instance in (1, 2):
        extensions = REPO / "config" / str(instance) / "asterisk" / "extensions.conf"
        text = extensions.read_text()
        head = text.split("[volte_ims_msg]", 1)[0]
        if "Wait(60)" in head or "[from-tg2sip]" not in text:
            text = _replace_context(text, "volte_ims", volte)
            if "[from-tg2sip]" not in text:
                if "[msg-from-sip]" in text:
                    text = text.replace("[msg-from-sip]", outbound + "\n[msg-from-sip]", 1)
                else:
                    text = text.rstrip() + "\n\n" + outbound
            extensions.write_text(text)
            changed_dialplan.append(instance)
        password = gateway_password(instance)
        pjsip = REPO / "config" / str(instance) / "asterisk" / "pjsip.conf"
        pjsip_text = pjsip.read_text()
        if "[tg2sip]" not in pjsip_text:
            block = pjsip_tg2sip_block(password)
            marker = ";===============VoLTE"
            if marker in pjsip_text:
                pjsip_text = pjsip_text.replace(marker, block + marker, 1)
            else:
                pjsip_text += "\n" + block
            pjsip.write_text(pjsip_text)
            changed_pjsip.append(instance)
        _ensure_compose(instance)
    restarted = []
    for instance in sorted(set(changed_dialplan) | set(changed_pjsip)):
        service = LINES[instance]["service"]
        if service in _service_states():
            result = _docker(["restart", service], timeout=90)
            if result.returncode == 0:
                restarted.append(service)
    return {
        "dialplan": changed_dialplan,
        "pjsip": changed_pjsip,
        "restarted": restarted,
    }


def _ensure_compose(instance: int) -> None:
    path = REPO / "compose.yaml"
    if not path.exists():
        return
    text = path.read_text()
    gateway = LINES[instance]["gateway"]
    if f"  {gateway}:" in text:
        return
    block = "\n".join(compose_service_lines(instance)) + "\n"
    marker = "\nvolumes:\n"
    if marker not in text:
        path.write_text(text.rstrip() + "\n" + block)
        return
    path.write_text(text.replace(marker, "\n" + block + marker, 1))


def self_check() -> dict[str, bool]:
    assert _valid_target("123456789")
    assert _valid_target("@alice")
    assert _valid_target("+8613800138000")
    assert _valid_target("13800138000")
    assert not _valid_target("alice")
    assert not _valid_target("1234567890123456")
    sample = "same => n,Dial(PJSIP/tg2sip,60,g)\n"
    updated = DIAL_RE.sub("same => n,Dial(PJSIP/+8613800138000@tg2sip,60,g)", sample)
    assert "+8613800138000@tg2sip" in updated
    try:
        from tg2sip_login import classify
    except ImportError:
        from scripts.tg2sip_login import classify
    kind, _pos = classify('Enter phone number or bot token: \nIs "+1" correct? (y/N): ')
    assert kind == "confirm"
    block = pjsip_tg2sip_block("secret")
    assert "password=secret" in block
    assert "direct_media=no" in block
    return {"ok": True}


def dispatch(req: dict[str, object]) -> dict[str, object]:
    cmd = str(req.get("cmd", "status"))
    if cmd == "status":
        return status()
    if cmd == "forward":
        return set_forward(int(req["instance"]), str(req.get("target", "")))
    if cmd == "routes":
        routes = req.get("routes", [])
        if not isinstance(routes, list):
            raise CtlError("routes must be a list")
        return set_routes(int(req["instance"]), routes)
    if cmd == "power":
        return set_power(int(req["instance"]), str(req.get("action", "")))
    if cmd == "session":
        return submit_session(int(req["instance"]), str(req.get("step", "")), str(req.get("value", "")))
    if cmd == "bootstrap":
        return sync_live_configs()
    if cmd == "self-check":
        return self_check()
    raise CtlError(f"unknown command {cmd}")


def main(argv: list[str]) -> int:
    try:
        if len(argv) > 1:
            req: dict[str, object] = {"cmd": argv[1]}
        else:
            req = json.load(sys.stdin)
        body = dispatch(req)
        json.dump(body, sys.stdout)
        sys.stdout.write("\n")
        return 0
    except CtlError as exc:
        json.dump({"error": str(exc), "status": exc.code}, sys.stdout)
        sys.stdout.write("\n")
        return 1
    except Exception as exc:  # noqa: BLE001 — surface a single JSON error to the API
        json.dump({"error": str(exc)}, sys.stdout)
        sys.stdout.write("\n")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
