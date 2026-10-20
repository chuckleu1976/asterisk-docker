#!/usr/bin/env python3
"""Restricted remote-PC/SC eSIM worker; hardware access is isolated in child processes."""

import argparse
from contextlib import contextmanager, redirect_stdout
from contextvars import ContextVar, copy_context
from dataclasses import dataclass
from email.parser import BytesParser
from email.policy import default as email_policy
import fcntl
import hashlib
import hmac
import ipaddress
from http.client import HTTPSConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import sqlite3
import ssl
import subprocess
import sys
import threading
import time
import tomllib
from urllib.parse import unquote, urlsplit, urlunsplit
from urllib.request import urlopen
import uuid
from urllib.error import HTTPError, URLError
from urllib.request import build_opener, HTTPRedirectHandler, HTTPSHandler, ProxyHandler, Request

ACTOR = ContextVar("gateway_actor", default=None)
ACTIVE_OPERATION = ContextVar("rental_operation", default=None)


class RentalHardware:
    def __init__(self, worker, runner):
        self.worker, self.runner = worker, runner

    def check(self):
        operation = ACTIVE_OPERATION.get()
        if operation:
            target, action, request_id = operation
            rental = self.worker.authorize(target, action)
            self.worker.management("POST", f"/{rental['id']}/reserve", {"request_id": request_id, "action": action})

    def lpac(self, target, arguments, body):
        self.check()
        return self.runner.lpac(target, arguments, body)

    def card(self, target, action, body):
        self.check()
        report = None
        if ACTOR.get() is not None and (self.worker.settings.get("management_api_url") or os.environ.get("MANAGEMENT_API_URL")):
            report = {"gateway_id": self.worker.settings.get("gateway_id") or os.environ.get("GATEWAY_ID", "asterisk-lan"),
                      "com_port": target.com_port, "bank_id": target.bank_id, "bank_slot": target.slot_nr,
                      "client_id": target.client_id, "client_slot": target.client_slot_nr}
            report.update(self.worker.management("POST", "/inventory/context", report))
        result = self.runner.card(target, action, body)
        if report is not None:
            try:
                self.worker.management("POST", "/inventory", {**report, "identity": result})
                result["inventory_sync"] = "updated"
            except OperationError:
                result["inventory_sync"] = "failed"
                result["warning"] = "Card readback succeeded, but management inventory refresh failed; do not repeat the card write"
        return result


class OperationError(Exception):
    def __init__(self, message, status=409):
        super().__init__(message)
        self.status = status


def smdp_host(value):
    if not isinstance(value, str):
        raise OperationError("Invalid SM-DP+ hostname", 400)
    host = value.strip().rstrip(".").lower()
    if not host or "%" in host:
        raise OperationError("Invalid SM-DP+ hostname", 400)
    try:
        return str(ipaddress.ip_address(host))
    except ValueError:
        pass
    try:
        host = host.encode("idna").decode("ascii")
    except UnicodeError:
        raise OperationError("Invalid SM-DP+ hostname", 400) from None
    if len(host) > 253 or any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label) for label in host.split(".")):
        raise OperationError("Invalid SM-DP+ hostname", 400)
    return host


def public_smdp_addresses(host):
    host = smdp_host(host)
    try:
        literal = ipaddress.ip_address(host)
    except ValueError:
        literal = None
    if literal is not None:
        addresses = [host]
    else:
        try:
            addresses = list(dict.fromkeys(item[4][0] for item in socket.getaddrinfo(host, 443, type=socket.SOCK_STREAM)))
        except OSError:
            raise OperationError("Unable to resolve SM-DP+ host", 502) from None
    if not addresses:
        raise OperationError("Unable to resolve SM-DP+ host", 502)
    for value in addresses:
        address = ipaddress.ip_address(value)
        if not address.is_global or address.is_multicast or address.is_reserved:
            raise OperationError("SM-DP+ host must resolve only to public unicast addresses", 403)
    return addresses


def activation_key(body):
    activation = body.get("activation_code")
    if activation:
        if not isinstance(activation, str):
            raise OperationError("Invalid activation code", 400)
        parts = activation.split("$")
        if len(parts) < 3 or parts[0] != "LPA:1":
            raise OperationError("Invalid activation code", 400)
        host, matching = parts[1:3]
    else:
        host, matching = body.get("smdp", ""), body.get("matching_id", "")
    if not isinstance(host, str) or not isinstance(matching, str) or not host or not matching or len(matching) > 4096:
        raise OperationError("SM-DP+ and matching ID are required", 400)
    return json.dumps([smdp_host(host), matching], separators=(",", ":"))


@dataclass(frozen=True)
class Target:
    id: str
    com_port: str
    bank_id: int
    slot_nr: int
    client_id: int
    client_slot_nr: int
    reader: str
    expected_eid: str = ""
    maintenance: bool = False
    protected: bool = True

    def __post_init__(self):
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", self.id):
            raise ValueError("Invalid target ID")
        if not self.reader or any(character in self.reader for character in "\r\n\0"):
            raise ValueError("An exact PC/SC reader name is required")
        if self.expected_eid and not re.fullmatch(r"[0-9]{32}", self.expected_eid):
            raise ValueError("Expected EID must contain 32 digits")
        if any(type(value) is not int or not 0 <= value <= 65535 for value in
               (self.bank_id, self.slot_nr, self.client_id, self.client_slot_nr)):
            raise ValueError("Invalid bank/client slot coordinates")


class Worker:
    def __init__(self, config, runner=None):
        self.settings = config["worker"]
        self.targets = [Target(**item) for item in config.get("targets", [])]
        for key in ("id", "com_port", "reader"):
            values = [getattr(target, key) for target in self.targets]
            if len(values) != len(set(values)):
                raise ValueError("Duplicate target " + key)
        for key in (lambda target: (target.bank_id, target.slot_nr),
                    lambda target: (target.client_id, target.client_slot_nr)):
            values = [key(target) for target in self.targets]
            if len(values) != len(set(values)):
                raise ValueError("Targets must not alias a bank or client slot")
        self.runner = runner or Hardware(self.settings)
        self.runner = RentalHardware(self, self.runner)
        self.lock = threading.Lock()
        self.session_lock = threading.Lock()
        self.session = None
        self.session_owner = None
        self.session_until = 0
        self.db_lock = threading.Lock()
        self.database = sqlite3.connect(self.settings.get("state_db", "remote-esim.sqlite3"), check_same_thread=False)
        self.database.execute("CREATE TABLE IF NOT EXISTS operations (id TEXT PRIMARY KEY, target TEXT, action TEXT, status TEXT, result TEXT)")
        self.database.execute("CREATE TABLE IF NOT EXISTS operation_owners (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, rental_id TEXT NOT NULL)")
        self.database.execute("CREATE TABLE IF NOT EXISTS download_attempts (fingerprint TEXT, operation_id TEXT, PRIMARY KEY (fingerprint, operation_id))")
        primary_key = [column[1] for column in self.database.execute("PRAGMA table_info(download_attempts)") if column[5]]
        if primary_key == ["fingerprint"]:
            with self.database:
                self.database.execute("BEGIN IMMEDIATE")
                self.database.execute("ALTER TABLE download_attempts RENAME TO download_attempts_legacy")
                self.database.execute("CREATE TABLE download_attempts (fingerprint TEXT, operation_id TEXT, PRIMARY KEY (fingerprint, operation_id))")
                self.database.execute("INSERT INTO download_attempts SELECT fingerprint, operation_id FROM download_attempts_legacy")
                self.database.execute("DROP TABLE download_attempts_legacy")
        self.database.execute("CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value BLOB)")
        self.database.execute("INSERT OR IGNORE INTO metadata VALUES ('fingerprint_key', ?)", (secrets.token_bytes(32),))
        self.fingerprint_key = self.database.execute("SELECT value FROM metadata WHERE key='fingerprint_key'").fetchone()[0]
        self.database.execute("UPDATE operations SET status='interrupted' WHERE status='running'")
        self.database.commit()
        self.job_lock = threading.Lock()
        self.job_changed = threading.Condition(self.job_lock)
        self.job = None
        self.job_owner = None
        self.job_rentals = set()
        self.batch_thread = None
        self.cancel = threading.Event()

    def management(self, method, path, body=None, credential=None):
        base = self.settings.get("management_api_url") or os.environ.get("MANAGEMENT_API_URL")
        if not base:
            raise OperationError("Rental authority is not configured", 503)
        actor = ACTOR.get()
        credential = credential or (actor and actor["credential"])
        if not credential:
            raise OperationError("Gateway credential required", 401)
        request = Request(base.rstrip("/") + "/rentals" + path,
                          data=json.dumps(body).encode() if body is not None else None,
                          headers={"Authorization": "Bearer " + credential, "Content-Type": "application/json",
                                   "X-Worker-Token": os.environ.get("REMOTE_ESIM_WORKER_TOKEN", "")}, method=method)
        try:
            with build_opener(ProxyHandler({}), NoRedirect()).open(request, timeout=5) as response:
                raw = response.read(262145)
            if len(raw) > 262144:
                raise OperationError("Rental authority response too large", 502)
            return json.loads(raw) if raw else None
        except HTTPError as error:
            raise OperationError("Rental access denied", error.code if error.code in (401, 403, 404, 409) else 503) from None
        except (URLError, OSError, ValueError):
            raise OperationError("Rental authority unavailable", 503) from None

    def identity(self, credential):
        configured = self.settings.get("management_api_url") or os.environ.get("MANAGEMENT_API_URL")
        if not configured:
            return None
        gateway = self.settings.get("gateway_id") or os.environ.get("GATEWAY_ID", "asterisk-lan")
        if not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", gateway):
            raise OperationError("Invalid Gateway identity", 503)
        context = self.management("GET", "/context?gateway_id=" + gateway, credential=credential)
        if not isinstance(context, dict) or not context.get("user_id") or not isinstance(context.get("rentals"), list):
            raise OperationError("Invalid rental identity", 502)
        return {"credential": credential, "context": context}

    def authorize(self, target, action):
        actor = ACTOR.get()
        if actor is None:
            return None
        identity = self.identity(actor["credential"])
        if identity is None:
            raise OperationError("Rental authority unavailable", 503)
        for rental in identity["context"]["rentals"]:
            if (rental["com_port"], rental["bank_id"], rental["bank_slot"], rental["client_id"], rental["client_slot"]) == (
                    target.com_port, target.bank_id, target.slot_nr, target.client_id, target.client_slot_nr):
                if action not in rental["permissions"]:
                    raise OperationError("Operation is not permitted by this rental or API token", 403)
                return rental
        raise OperationError("Remote SIM target not found in active rentals", 404)

    def permission(self, action, body):
        permissions = {"profiles/download": "esim:download", "profiles/enable": "esim:enable",
                       "profiles/disable": "esim:disable", "profiles/delete": "esim:delete", "msisdn": "esim:msisdn"}
        if action == "provision":
            return "esim:download"
        if action in ("profiles/nickname", "notifications/process", "notifications/remove"):
            if ACTOR.get()["context"].get("is_admin"):
                return "esim:enable"
            raise OperationError("This operation requires administrative maintenance", 403)
        return permissions.get(action, "esim:read")

    def target(self, identifier):
        found = [target for target in self.targets if identifier in (target.id, target.com_port)]
        if len(found) != 1:
            raise OperationError("Unknown remote SIM target", 404)
        return found[0]

    def mappings(self):
        with urlopen(self.settings["slotmaps_url"], timeout=5) as response:
            raw = response.read(262145)
        if len(raw) > 262144:
            raise OperationError("Mapping response is too large", 502)
        data = json.loads(raw)
        if not isinstance(data.get("slotmaps"), list):
            raise OperationError("Invalid mapping response", 502)
        return data["slotmaps"]

    def describe(self, target, mappings, error=None):
        rows = [row for row in mappings if row.get("client") ==
                {"clientId": target.client_id, "slotNr": target.client_slot_nr}]
        row = rows[0] if len(rows) == 1 else None
        expected = {"bankId": target.bank_id, "slotNr": target.slot_nr}
        correct = row is not None and row.get("bank") == expected
        state = "UNAVAILABLE" if error else ("UNMAPPED" if not rows else
                ("MISMATCH" if not correct else row.get("state", "UNKNOWN")))
        actual = row.get("bank", {}) if row else {}
        slot_label = f"Bank {actual['bankId']} / Slot {actual['slotNr']}" if actual else "Unmapped"
        mapping_label = (f"B{actual['bankId']}:{actual['slotNr']} -> C{target.client_id}:{target.client_slot_nr}"
                         if actual else "Unmapped")
        if error:
            slot_label = mapping_label = "Unknown"
        with self.session_lock:
            active = self.session == target.id and self.session_until > time.monotonic()
        return {"id": target.id, "com_port": target.com_port, "bank_id": target.bank_id,
                "slot_nr": target.slot_nr, "client_id": target.client_id,
                "client_slot_nr": target.client_slot_nr, "reader": target.reader,
                "slot_label": slot_label, "mapping_label": mapping_label, "state": state,
                "error": error, "esim_capable": bool(target.expected_eid) and state == "ACTIVE" and target.maintenance and not target.protected,
                "esim_mode": active, "session_active": active, "sim_id": None,
                "protected": target.protected, "maintenance": target.maintenance,
                "writable": bool(self.settings.get("mutations_enabled", False)) and
                            target.maintenance and not target.protected and bool(target.expected_eid) and state == "ACTIVE"}

    def inventory(self):
        try:
            mappings, error = self.mappings(), None
        except Exception:
            mappings, error = [], "Live mapping service unavailable"
        rows = []
        for target in self.targets:
            rental = None
            try:
                rental = self.authorize(target, "esim:read")
            except OperationError as denied:
                if denied.status in (403, 404):
                    if ACTOR.get() is None or not ACTOR.get()["context"].get("is_admin"):
                        continue
                else:
                    raise
            row = self.describe(target, mappings, error)
            if ACTOR.get() is not None:
                row.pop("reader", None)
                row["permissions"] = rental["permissions"] if rental else []
            rows.append(row)
        return rows

    def guard(self, target, write=False):
        try:
            state = self.describe(target, self.mappings())
        except OperationError:
            raise
        except Exception:
            raise OperationError("Cannot verify live mapping", 503) from None
        if state["state"] != "ACTIVE":
            raise OperationError("Target mapping is " + state["state"])
        if not target.expected_eid:
            raise OperationError("Target has no pinned EID")
        if target.protected or not target.maintenance:
            raise OperationError("Card access requires an unprotected target in maintenance", 403)
        if write and not state["writable"]:
            raise OperationError("Mutation requires an unprotected target in authorized maintenance", 403)
        return state

    def enter(self, target):
        rental = self.authorize(target, "esim:read")
        if self.lock.locked():
            raise OperationError("A card operation is running")
        self.guard(target)
        with self.session_lock:
            if self.session and self.session != target.id and self.session_until > time.monotonic():
                raise OperationError("Another remote SIM session is active")
            self.session, self.session_until = target.id, time.monotonic() + 900
            self.session_owner = (ACTOR.get()["context"]["user_id"], rental["id"]) if rental else None
        return {"com_port": target.com_port, "esim_mode": True}

    def exit(self, target):
        rental = self.authorize(target, "esim:read")
        if self.lock.locked():
            raise OperationError("A card operation is running; wait for its result")
        with self.session_lock:
            if rental and self.session_owner != (ACTOR.get()["context"]["user_id"], rental["id"]):
                raise OperationError("Session not found", 404)
            if self.session == target.id:
                self.session = None
                self.session_owner = None
        return {"com_port": target.com_port, "esim_mode": False}

    def operation(self, target, action, body):
        actor = ACTOR.get()
        if actor is None:
            return self._operation(target, action, body)
        permission = self.permission(action, body)
        rental = self.authorize(target, permission)
        if action == "provision":
            self.authorize(target, "esim:enable")
            if body.get("phone_number"):
                self.authorize(target, "esim:msisdn")
        request_id = body.get("request_id")
        if request_id:
            self.owned_operation(request_id, rental)
        reservation = str(uuid.uuid4())
        self.management("POST", f"/{rental['id']}/reserve", {"request_id": reservation, "action": permission})
        marker = ACTIVE_OPERATION.set((target, permission, reservation))
        try:
            return self._operation(target, action, body)
        finally:
            ACTIVE_OPERATION.reset(marker)
            try:
                self.management("DELETE", f"/{rental['id']}/reserve/{reservation}")
            except OperationError:
                pass

    def owned_operation(self, request_id, rental=None):
        actor = ACTOR.get()
        if actor is None:
            return
        with self.db_lock:
            owner = self.database.execute("SELECT user_id, rental_id FROM operation_owners WHERE id=?", (request_id,)).fetchone()
            exists = self.database.execute("SELECT 1 FROM operations WHERE id=?", (request_id,)).fetchone()
        if exists:
            current = self.identity(actor["credential"])["context"]["rentals"]
            if not owner or owner[0] != actor["context"]["user_id"] or not any(row["id"] == owner[1] for row in current):
                raise OperationError("Unknown operation", 404)
            if rental and rental["id"] != owner[1]:
                raise OperationError("Unknown operation", 404)

    def _operation(self, target, action, body):
        rental = self.authorize(target, self.permission(action, body)) if ACTOR.get() is not None else None
        write = action not in ("chip", "profiles", "notifications", "identity")
        request_id = body.get("request_id")
        if write:
            try:
                uuid.UUID(request_id)
            except (ValueError, TypeError, AttributeError):
                raise OperationError("A UUID request_id is required for mutations", 400) from None
        with self.job_lock:
            if self.job and self.job["status"] == "running" and threading.current_thread() is not self.batch_thread:
                raise OperationError("A batch owns the remote PC/SC worker")
            if not self.lock.acquire(blocking=False):
                raise OperationError("Remote PC/SC worker is busy")
        recorded = False
        try:
            with self.session_lock:
                if self.session and self.session != target.id and self.session_until > time.monotonic():
                    raise OperationError("Another remote SIM session owns the worker")
            self.guard(target, write)
            if write:
                with self.db_lock:
                    prior = self.database.execute("SELECT target, action, status, result FROM operations WHERE id=?", (request_id,)).fetchone()
                    if prior:
                        if prior[0:2] != (target.id, action):
                            raise OperationError("request_id belongs to another operation")
                        if prior[2] == "completed":
                            return json.loads(prior[3])
                        raise OperationError("Operation already attempted; inspect status before retrying")
                    if action in ("profiles/download", "provision"):
                        activation = activation_key(body)
                        host = json.loads(activation)[0]
                        public_smdp_addresses(host)
                        fingerprint = hmac.new(self.fingerprint_key, activation.encode(), hashlib.sha256).hexdigest()
                        self.database.execute("INSERT INTO download_attempts VALUES (?, ?)", (fingerprint, request_id))
                    self.database.execute("INSERT INTO operations VALUES (?, ?, ?, 'running', NULL)", (request_id, target.id, action))
                    if ACTOR.get() is not None:
                        self.database.execute("INSERT INTO operation_owners VALUES (?, ?, ?)",
                                              (request_id, ACTOR.get()["context"]["user_id"], rental["id"]))
                    self.database.commit()
                    recorded = True
            result = self.perform(target, action, body)
            self.guard(target)
            if recorded:
                with self.db_lock:
                    self.database.execute("UPDATE operations SET status='completed', result=? WHERE id=?", (json.dumps(result), request_id))
                    self.database.commit()
            return result
        except Exception as error:
            if recorded:
                with self.db_lock:
                    result = {"error": str(error) if isinstance(error, OperationError) else "Remote operation failed"}
                    self.database.execute("UPDATE operations SET status='failed', result=? WHERE id=?", (json.dumps(result), request_id))
                    self.database.commit()
            raise
        finally:
            self.lock.release()

    def profile_list(self, target):
        profiles = self.runner.lpac(target, ["profile", "list"], {})
        if not isinstance(profiles, list) or any(not isinstance(profile, dict) for profile in profiles):
            raise OperationError("Invalid lpac profile inventory", 502)
        return profiles

    def profile_operation(self, target, action, body):
        iccid = str(body.get("iccid", ""))
        if not re.fullmatch(r"[0-9]{18,22}", iccid):
            raise OperationError("An exact profile ICCID is required", 400)
        matches = [profile for profile in self.profile_list(target) if profile.get("iccid") == iccid]
        if len(matches) != 1:
            raise OperationError("Requested profile is not present or is ambiguous")
        state = str(matches[0].get("profileState", "")).lower()
        command = action.split("/")[-1]
        arguments = ["profile", command, iccid]
        if command in ("enable", "disable"):
            desired = "enabled" if command == "enable" else "disabled"
            if state == desired:
                return {"iccid": iccid, "state": desired, "verified": True}
            arguments.append("0")
        elif command == "delete":
            if state != "disabled":
                raise OperationError("Disable the profile explicitly before deleting it")
        elif command == "nickname":
            nickname = body.get("nickname", "")
            if not isinstance(nickname, str) or len(nickname.encode()) > 64:
                raise OperationError("Invalid profile nickname", 400)
            arguments.append(nickname)
        else:
            raise OperationError("Unsupported profile operation", 400)
        self.guard(target, True)
        self.runner.lpac(target, arguments, {})
        self.guard(target, True)
        after = [profile for profile in self.profile_list(target) if profile.get("iccid") == iccid]
        if command == "delete":
            if after:
                raise OperationError("Profile deletion was not verified", 502)
            return {"iccid": iccid, "deleted": True}
        if len(after) != 1:
            raise OperationError("Profile readback is missing or ambiguous", 502)
        if command == "nickname":
            if (after[0].get("profileNickname") or "") != nickname:
                raise OperationError("Profile nickname readback failed", 502)
            return {"iccid": iccid, "nickname": nickname}
        if str(after[0].get("profileState", "")).lower() != desired:
            raise OperationError("Profile state readback failed", 502)
        return {"iccid": iccid, "state": desired, "verified": True}

    def perform(self, target, action, body):
        if action == "chip":
            return self.runner.lpac(target, ["chip", "info"], {})
        if action == "profiles":
            return self.profile_list(target)
        if action in ("profiles/enable", "profiles/disable", "profiles/delete", "profiles/nickname"):
            return self.profile_operation(target, action, body)
        if action == "notifications":
            return self.runner.lpac(target, ["notification", "list"], {})
        if action in ("profiles/download", "provision"):
            before = self.profile_list(target)
            self.guard(target, True)
            self.runner.lpac(target, ["profile", "download"], body)
            self.guard(target, True)
            after = self.profile_list(target)
            new = [profile for profile in after if profile["iccid"] not in {old["iccid"] for old in before}]
            if len(new) != 1:
                raise OperationError("Download finished but new profile is ambiguous; inspect inventory", 502)
            iccid = new[0]["iccid"]
            result = {"iccid": iccid, "downloaded": True, "enabled": False, "stages": {"download": "verified"}}
            if action == "provision":
                try:
                    self.guard(target, True)
                    self.profile_operation(target, "profiles/enable", {"iccid": iccid})
                    result["enabled"] = True
                    result["stages"]["enable"] = "verified"
                    result["identity"] = self.runner.card(target, "identity", {"iccid": iccid})
                    if body.get("phone_number"):
                        self.guard(target, True)
                        result["msisdn"] = self.runner.card(target, "msisdn", {"iccid": iccid, "phone_number": body["phone_number"]})
                        result["stages"]["msisdn"] = "verified_on_card"
                except OperationError as error:
                    result["partial"] = True
                    result["warning"] = str(error)
                    result["stages"]["identity_or_msisdn" if result["enabled"] else "enable"] = "failed"
            try:
                notifications = self.runner.lpac(target, ["notification", "list"], {}) or []
                for notification in notifications:
                    if notification.get("iccid") == iccid:
                        self.guard(target, True)
                        self.runner.lpac(target, ["notification", "process", "-r", str(notification["seqNumber"])], {})
                result["stages"]["notifications"] = "processed"
            except OperationError as error:
                result["partial"] = True
                result["notification_warning"] = str(error)
            return result
        if action in ("notifications/process", "notifications/remove"):
            sequence = str(body.get("seq", ""))
            if not re.fullmatch(r"[0-9]{1,10}", sequence) or int(sequence) > 4294967295:
                raise OperationError("Invalid notification sequence", 400)
            sequence = str(int(sequence))
            if action == "notifications/remove":
                self.runner.lpac(target, ["notification", "remove", sequence], {})
                self.guard(target, True)
                notifications = self.runner.lpac(target, ["notification", "list"], {}) or []
                if any(str(item.get("seqNumber")) == sequence for item in notifications):
                    raise OperationError("Notification removal was not verified", 502)
                return {"seqNumber": int(sequence), "removed": True}
            arguments = ["notification", "process"] + (["-r"] if body.get("remove") else []) + [sequence]
            return self.runner.lpac(target, arguments, {})
        if action not in ("identity", "msisdn"):
            raise OperationError("Unsupported card operation", 400)
        matches = [profile for profile in self.profile_list(target) if profile.get("iccid") == body.get("iccid")]
        if len(matches) != 1 or str(matches[0].get("profileState", "")).lower() != "enabled":
            raise OperationError("Requested profile must be enabled for identity access")
        return self.runner.card(target, action, body)

    def operation_status(self, request_id):
        self.owned_operation(request_id)
        with self.db_lock:
            row = self.database.execute("SELECT target, action, status, result FROM operations WHERE id=?", (request_id,)).fetchone()
        if not row:
            raise OperationError("Unknown operation", 404)
        self.authorize(self.target(row[0]), "esim:read")
        return {"request_id": request_id, "target": row[0], "action": row[1], "status": row[2],
                "result": json.loads(row[3]) if row[3] else None}

    def start_batch(self, body):
        if body.get("replace_existing"):
            raise OperationError("Remote jobs never replace or delete profiles implicitly", 400)
        operation = body.get("op")
        items = body.get("items")
        if operation not in ("download", "activate", "deactivate", "delete") or not isinstance(items, list) or not 1 <= len(items) <= 64:
            raise OperationError("Invalid batch request", 400)
        targets = [self.target(item.get("com_port")) for item in items]
        permission = {"download": "esim:download", "activate": "esim:enable", "deactivate": "esim:disable", "delete": "esim:delete"}[operation]
        rentals = []
        for target, item in zip(targets, items):
            rental = self.authorize(target, permission)
            if rental:
                rentals.append(rental["id"])
            if operation == "download" and body.get("auto_enable"):
                self.authorize(target, "esim:enable")
                if item.get("phone_number"):
                    self.authorize(target, "esim:msisdn")
        if len({target.id for target in targets}) != len(targets):
            raise OperationError("Duplicate batch targets", 400)
        if operation == "download":
            for item in items:
                host = json.loads(activation_key(item))[0]
                public_smdp_addresses(host)
        with self.job_lock:
            if self.job and self.job["status"] == "running":
                raise OperationError("A remote eSIM batch is already running")
            if self.lock.locked():
                raise OperationError("A card operation is already running")
            self.cancel.clear()
            self.job_owner = ACTOR.get()["context"]["user_id"] if ACTOR.get() else None
            self.job_rentals = set(rentals)
            self.job = {"id": str(uuid.uuid4()), "op": operation, "status": "running",
                        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "finished_at": None,
                        "items": [{"com_port": target.com_port, "status": "pending", "message": None, "iccid": None}
                                  for target in targets]}
            snapshot = json.loads(json.dumps(self.job))
            self.batch_thread = threading.Thread(target=copy_context().run, args=(self.run_batch, body, targets), daemon=True)
        self.batch_thread.start()
        return snapshot

    def run_batch(self, body, targets):
        stopped = False
        for index, (target, item) in enumerate(zip(targets, body["items"])):
            with self.job_changed:
                row = self.job["items"][index]
                if self.cancel.is_set() or stopped:
                    row.update(status="skipped", message="Cancelled or stopped after an error")
                    self.job_changed.notify_all()
                    continue
                row["status"] = "running"
                self.job_changed.notify_all()
            try:
                request = {**item, "request_id": str(uuid.uuid4())}
                if body["op"] == "download":
                    action = "provision" if body.get("auto_enable", False) else "profiles/download"
                else:
                    profiles = self.operation(target, "profiles", {})
                    if len(profiles) != 1:
                        raise OperationError("Batch profile selection is ambiguous; select an ICCID individually")
                    request["iccid"] = profiles[0]["iccid"]
                    action = {"activate": "profiles/enable", "deactivate": "profiles/disable", "delete": "profiles/delete"}[body["op"]]
                result = self.operation(target, action, request)
                stopped = bool(result.get("partial")) and bool(body.get("stop_on_error", True))
                with self.job_changed:
                    row.update(status="failed" if result.get("partial") else "ok",
                               iccid=result.get("iccid"), message=result.get("warning") or result.get("notification_warning") or "Verified")
            except Exception as error:
                with self.job_changed:
                    row.update(status="failed", message=str(error) if isinstance(error, OperationError) else "Remote operation failed")
                stopped = bool(body.get("stop_on_error", True))
            with self.job_changed:
                self.job_changed.notify_all()
        with self.job_changed:
            self.job["status"] = "cancelled" if self.cancel.is_set() else "done"
            self.job["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            self.job_changed.notify_all()

    def snapshot(self):
        actor = ACTOR.get()
        context = self.identity(actor["credential"])["context"] if actor is not None else None
        with self.job_lock:
            if context is not None:
                readable = {row["id"] for row in context["rentals"] if "esim:read" in row["permissions"]}
                if self.job_owner != context["user_id"] or not self.job_rentals <= readable:
                    return None
            return json.loads(json.dumps(self.job))

    def sources(self):
        if ACTOR.get() is not None and not ACTOR.get()["context"].get("is_admin"):
            return []
        directory = self.settings.get("source_dir")
        if not directory:
            return []
        result = []
        for path in sorted(Path(directory).iterdir()):
            if path.is_file() and not path.is_symlink() and path.stat().st_size <= 4 * 1024 * 1024:
                result.extend(parse_source(path.name, path.read_bytes(), self.settings))
        return result


class Hardware:
    def __init__(self, settings):
        self.settings = settings

    def command(self, mode):
        command = self.settings.get("hardware_command", [sys.executable, str(Path(__file__).resolve())])
        if not isinstance(command, list) or not command or any(not isinstance(argument, str) or not argument for argument in command):
            raise OperationError("Invalid configured hardware command", 503)
        return [*command, mode]

    def card(self, target, action, body):
        request = {"target": target.__dict__, "action": action, "body": body}
        try:
            result = subprocess.run(self.command("--card"),
                                    input=json.dumps(request), capture_output=True, text=True, timeout=45)
        except subprocess.TimeoutExpired:
            raise OperationError("Card operation timed out; inspect card state before retrying", 504) from None
        try:
            reply = json.loads(result.stdout)
        except (ValueError, TypeError):
            raise OperationError("pySim worker failed; check runtime dependencies", 502) from None
        if result.returncode or not reply.get("success"):
            raise OperationError(reply.get("error", "pySim operation failed"), 502)
        return reply["data"]

    def https(self, payload):
        raw_url = payload["url"]
        try:
            if not isinstance(raw_url, str) or any(ord(character) < 32 or ord(character) == 127 for character in raw_url):
                raise ValueError()
            url = urlsplit(raw_url)
            if url.scheme != "https" or not url.hostname or url.port not in (None, 443) or url.username or url.password or url.fragment:
                raise ValueError()
        except ValueError:
            raise OperationError("Carrier URL must use HTTPS on port 443 without credentials or fragments", 403) from None
        host = smdp_host(url.hostname)
        addresses = public_smdp_addresses(host)
        context = ssl.create_default_context()
        if self.settings.get("ca_file"):
            context.load_verify_locations(cafile=self.settings["ca_file"])
        headers = {}
        for header in payload.get("headers", []):
            name, value = header.split(":", 1)
            headers[name.strip()] = value.strip()
        authority = "[" + host + "]" if ":" in host else host
        request_url = urlunsplit(("https", authority, url.path, url.query, ""))
        request = Request(request_url, data=bytes.fromhex(payload.get("tx") or ""), headers=headers, method="POST")
        opener = build_opener(ProxyHandler({}), PublicHTTPSHandler(addresses, context=context), NoRedirect())
        try:
            response = opener.open(request, timeout=25)
        except HTTPError as error:
            response = error
        except URLError as error:
            if isinstance(error.reason, ssl.SSLCertVerificationError):
                raise OperationError("Carrier TLS certificate verification failed; check the approved CA bundle", 502) from None
            raise OperationError("Carrier HTTPS connection failed", 502) from None
        with response:
            raw = response.read(8 * 1024 * 1024 + 1)
            if len(raw) > 8 * 1024 * 1024:
                raise OperationError("Carrier response exceeds 8 MiB", 502)
            return {"type": "http", "payload": {"rcode": response.code, "rx": raw.hex()}}

    def lpac(self, target, arguments, body):
        request = {"target": target.__dict__, "settings": self.settings, "arguments": arguments, "body": body}
        with subprocess.Popen(self.command("--lpac"),
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                              text=True, start_new_session=True) as process:
            try:
                output, _ = process.communicate(json.dumps(request), timeout=270)
                reply = json.loads(output)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
                raise OperationError("Download timed out; inspect operation and card state before retrying", 504) from None
            except (ValueError, TypeError):
                raise OperationError("lpac worker failed; inspect runtime dependencies", 502) from None
        if process.returncode or not reply.get("success"):
            raise OperationError(reply.get("error", "lpac operation failed"), 502)
        return reply["data"]

    def lpac_stdio(self, target, arguments, body, bridge):
        activation = body.get("activation_code")
        secrets = [str(value) for key, value in body.items() if key in ("activation_code", "matching_id", "confirmation_code") and value]
        if arguments[:2] == ["profile", "download"]:
            host = json.loads(activation_key(body))[0]
            public_smdp_addresses(host)
            if activation:
                parts = activation.split("$")
                if len(parts) < 3 or parts[0] != "LPA:1" or not parts[2]:
                    raise OperationError("Invalid activation code", 400)
                parts[1] = host
                secrets.append(parts[2])
                arguments += ["-a", "$".join(parts)]
            else:
                if not body.get("matching_id"):
                    raise OperationError("Activation code or SM-DP+/matching ID is required", 400)
                arguments += ["-s", host, "-m", body["matching_id"]]
            for field, option in (("confirmation_code", "-c"), ("imei", "-i")):
                if body.get(field):
                    arguments += [option, body[field]]
        environment = os.environ.copy()
        for key in list(environment):
            if key.startswith("LPAC_"):
                del environment[key]
        environment.update(LPAC_APDU="stdio", LPAC_HTTP="stdio")
        process = subprocess.Popen([self.settings["lpac_exe"]] + arguments, env=environment,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        timer = threading.Timer(240, process.kill)
        timer.start()
        reply = None
        try:
            for line in process.stdout:
                if len(line) > 20 * 1024 * 1024:
                    raise OperationError("lpac response exceeds size limit", 502)
                message = json.loads(line)
                if message.get("type") == "http":
                    response = self.https(message["payload"])
                    process.stdin.write(json.dumps(response) + "\n")
                    process.stdin.flush()
                elif message.get("type") == "apdu":
                    response = bridge(message["payload"])
                    process.stdin.write(json.dumps(response) + "\n")
                    process.stdin.flush()
                elif message.get("type") == "lpa":
                    reply = message["payload"]
            code = process.wait(timeout=5)
            if code or not reply or reply.get("code") != 0:
                stage = (reply or {}).get("message", "lpac failed or timed out")
                detail = (reply or {}).get("data")
                message = str(stage) + (": " + detail if isinstance(detail, str) else "")
                if str(stage).startswith("es9p_") and detail == "Refused":
                    message = "Carrier response (SM-DP+): " + message
                for secret in secrets:
                    message = message.replace(secret, "[REDACTED]")
                raise OperationError(message[:500], 502)
            return reply.get("data")
        except (ValueError, OSError, ssl.SSLError):
            raise OperationError("Verified carrier HTTPS or lpac transport failed", 502) from None
        finally:
            timer.cancel()
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stdin.close()
            process.stdout.close()


class PublicHTTPSConnection(HTTPSConnection):
    def __init__(self, host, *, addresses, **kwargs):
        super().__init__(host, **kwargs)
        self.addresses = addresses

    def connect(self):
        last_error = OSError("No public carrier address is reachable")
        for address in self.addresses:
            transport = socket.socket(socket.AF_INET6 if ":" in address else socket.AF_INET, socket.SOCK_STREAM)
            try:
                transport.settimeout(self.timeout)
                if self.source_address:
                    transport.bind(self.source_address)
                transport.connect((address, self.port))
            except OSError as error:
                transport.close()
                last_error = error
                continue
            try:
                self.sock = self._context.wrap_socket(transport, server_hostname=self.host)
            except Exception:
                transport.close()
                raise
            return
        raise last_error


class PublicHTTPSHandler(HTTPSHandler):
    def __init__(self, addresses, **kwargs):
        super().__init__(**kwargs)
        self.addresses = addresses

    def https_open(self, request):
        def connection(host, **kwargs):
            return PublicHTTPSConnection(host, addresses=self.addresses, **kwargs)
        return self.do_open(connection, request, context=self._context)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise OperationError("Carrier redirect refused", 502)


@contextmanager
def pinned_card(target):
    from pySim.commands import SimCardCommands
    from pySim.euicc import AID_ISD_R, CardApplicationISDR
    from pySim.transport.pcsc import PcscSimLink

    transport = PcscSimLink(argparse.Namespace(pcsc_dev=None, pcsc_regex="^" + re.escape(target.reader) + "$", pcsc_shared=False))
    base = SimCardCommands(transport)
    base.cla_byte = "00"
    base.sel_ctrl = "0004"
    opened = False
    try:
        transport.connect()
        base.manage_channel(mode="open", lchan_nr=2)
        opened = True
        commands = base.fork_lchan(2)
        commands.select_adf(AID_ISD_R)
        if CardApplicationISDR.get_eid(commands) != target.expected_eid:
            raise OperationError("EID mismatch; download refused")
        transport.apdu_strict = False

        def bridge(payload):
            function = payload.get("func")
            result = {"ecode": 0}
            if function == "logic_channel_open":
                if str(payload.get("param", "")).lower() != AID_ISD_R.lower():
                    raise OperationError("lpac requested an unexpected application")
                result["ecode"] = 2
            elif function == "transmit":
                data, status = transport.send_apdu(payload["param"])
                result["data"] = data + status
            elif function not in ("connect", "disconnect", "logic_channel_close"):
                raise OperationError("Unsupported lpac APDU operation")
            return {"type": "apdu", "payload": result}

        yield bridge
    finally:
        try:
            if opened:
                base.manage_channel(mode="close", lchan_nr=2)
        finally:
            transport.disconnect()


def msisdn_record(record, size, phone):
    from pySim.ts_51_011 import EF_MSISDN

    if not isinstance(phone, str) or not re.fullmatch(r"\+?[0-9]{1,20}", phone):
        raise OperationError("MSISDN must contain explicit digits; no country code is inferred")
    if size < 14 or len(record) != size * 2 or record[-4:].lower() != "ffff":
        raise OperationError("Invalid MSISDN record or extension references; manual handling required")
    codec = EF_MSISDN()
    previous = codec.decode_record_hex(record, 1)
    digits = phone.lstrip("+")
    return codec.encode_record_hex({"alpha_id": previous.get("alpha_id", ""),
                                    "len_of_bcd": (len(digits) + 1) // 2 + 1,
                                    "ton_npi": {"ext": True, "type_of_number": "international" if phone.startswith("+") else "network_specific",
                                                "numbering_plan_id": "isdn_e164"},
                                    "dialing_nr": digits}, 1, total_len=size)


def msisdn_access(commands, path):
    from pySim.ts_102_221 import CardProfileUICC, EF_ARR

    result = {"update": "unknown"}
    try:
        fcp = CardProfileUICC.decode_select_response(commands.select_file(path)[0])
        reference = fcp.get("security_attrib_referenced")
        if not reference:
            return result
        file_id = reference["ef_arr_file_id"]
        file_id = file_id.hex() if isinstance(file_id, bytes) else str(file_id)
        record_nr = reference["ef_arr_record_nr"]
        result.update(arr_file=file_id, arr_record=record_nr)
        if not re.fullmatch(r"6f[0-9a-fA-F]{2}", file_id):
            return result
        raw = commands.read_record(file_id, record_nr)[0]
        rules = EF_ARR.flatten(EF_ARR().decode_record_hex(raw, record_nr))
        rule = rules.get("update_record", rules.get("update_erase", "unknown"))
        result["update"] = rule if isinstance(rule, str) else "conditional"
    except Exception as error:
        result["error"] = "Access rules could not be read"
        status = getattr(error, "sw_actual", None)
        if isinstance(status, str):
            result["sw"] = status
    return result


def card_action(target, action, body):
    if action not in ("identity", "msisdn"):
        raise OperationError("pySim only handles identity and MSISDN access", 400)
    iccid = str(body.get("iccid", ""))
    if not re.fullmatch(r"[0-9]{18,22}", iccid):
        raise OperationError("An exact profile ICCID is required", 400)
    from pySim.commands import SimCardCommands
    from pySim.cards import UiccCardBase
    import pySim.euicc as euicc
    from pySim.transport.pcsc import PcscSimLink
    from pySim.ts_51_011 import EF_MSISDN
    from pySim.utils import dec_iccid, dec_imsi

    transport = PcscSimLink(argparse.Namespace(pcsc_dev=None, pcsc_regex="^" + re.escape(target.reader) + "$", pcsc_shared=False))
    base = SimCardCommands(transport)
    base.cla_byte = "00"
    base.sel_ctrl = "0004"
    opened = False
    stage = "PC/SC connection"
    try:
        transport.connect()
        stage = "logical channel opening"
        base.manage_channel(mode="open", lchan_nr=2)
        opened = True
        commands = base.fork_lchan(2)
        stage = "ISD-R selection"
        commands.select_adf(euicc.AID_ISD_R)
        stage = "EID verification"
        eid = euicc.CardApplicationISDR.get_eid(commands)
        if eid != target.expected_eid:
            raise OperationError("EID mismatch; operation refused")

        stage = "ISD-R channel close before SIM file access"
        base.manage_channel(mode="close", lchan_nr=2)
        opened = False
        stage = "SIM file channel opening"
        base.manage_channel(mode="open", lchan_nr=2)
        opened = True
        commands = base.fork_lchan(2)
        stage = "MF selection"
        commands.select_file("3f00")
        stage = "active ICCID read"
        actual_iccid = dec_iccid(commands.read_binary("2fe2")[0])
        if actual_iccid != iccid:
            raise OperationError("Active ICCID mismatch; no MSISDN write attempted")
        stage = "USIM application discovery"
        card = UiccCardBase(commands)
        card.read_aids()
        stage = "USIM selection"
        card.select_adf_by_aid("usim")
        stage = "IMSI read"
        imsi = dec_imsi(commands.read_binary("6f07")[0])
        result = {"iccid": iccid, "imsi": imsi, "msisdn_on_card": "not_read"}
        path = "6f40"
        try:
            stage = "USIM EF.MSISDN selection"
            size = commands.record_size(path)
        except Exception:
            stage = "Telecom selection"
            commands.select_path(["3f00", "7f10"])
            stage = "Telecom EF.MSISDN selection"
            size = commands.record_size(path)
        stage = "EF.MSISDN record read"
        record = commands.read_record(path, 1)[0]
        codec = EF_MSISDN()
        if action == "msisdn":
            phone = body.get("phone_number", "")
            encoded = msisdn_record(record, size, phone)
            digits = phone.lstrip("+")
            access = msisdn_access(commands, path)
            rule = access["update"]
            if rule == "never":
                raise OperationError("This profile forbids EF.MSISDN updates; no card write attempted")
            if rule in ("unknown", "conditional"):
                raise OperationError("EF.MSISDN write permission could not be established; no card write attempted. Inspect the profile access rules")
            if rule != "always":
                raise OperationError("EF.MSISDN write requires " + rule + " authentication from the profile issuer; no card write attempted. Web rental permissions do not unlock this file")
            stage = "EF.MSISDN update and verification"
            commands.update_record(path, 1, encoded, verify=True)
            stage = "EF.MSISDN readback"
            record = commands.read_record(path, 1)[0]
            decoded = codec.decode_record_hex(record, 1)
            if decoded.get("dialing_nr", "").rstrip("fF") != digits:
                raise OperationError("MSISDN readback did not match")
        decoded = codec.decode_record_hex(record, 1)
        result.update(msisdn=decoded.get("dialing_nr", "").rstrip("fF"), msisdn_on_card="verified" if action == "msisdn" else "read")
        if action == "identity":
            result["msisdn_access"] = msisdn_access(commands, path)
        return result
    except Exception as error:
        status = getattr(error, "sw_actual", None)
        if status:
            reason = "Card instruction not supported" if str(status).lower() == "6d00" else "Card command failed"
            raise OperationError(reason + " during " + stage + " (SW=" + str(status) + ")", 502) from None
        raise
    finally:
        try:
            if opened:
                base.manage_channel(mode="close", lchan_nr=2)
        finally:
            transport.disconnect()


def card_main():
    try:
        request = json.load(sys.stdin)
        with redirect_stdout(sys.stderr):
            result = card_action(Target(**request["target"]), request["action"], request["body"])
        print(json.dumps({"success": True, "data": result}, default=lambda value: value.hex() if isinstance(value, bytes) else str(value)))
    except OperationError as error:
        print(json.dumps({"success": False, "error": str(error)}))
        raise SystemExit(1)
    except Exception as error:
        status = getattr(error, "sw_actual", None)
        message = "Card access denied or unavailable" + (" (SW=" + status + ")" if status else "")
        print(json.dumps({"success": False, "error": message}))
        raise SystemExit(1)


def lpac_main():
    try:
        request = json.load(sys.stdin)
        target = Target(**request["target"])
        with redirect_stdout(sys.stderr), pinned_card(target) as bridge:
            result = Hardware(request["settings"]).lpac_stdio(target, request["arguments"], request["body"], bridge)
        print(json.dumps({"success": True, "data": result}))
    except OperationError as error:
        print(json.dumps({"success": False, "error": str(error)}))
        raise SystemExit(1)
    except Exception:
        print(json.dumps({"success": False, "error": "lpac/card transport unavailable; inspect operation before retrying"}))
        raise SystemExit(1)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def send_json(self, status, data):
        raw = json.dumps(data).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.send_header("Connection", "close")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(raw)

    def handle_request(self):
        self.close_connection = True
        self.connection.settimeout(15)
        token = self.server.token
        if not hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token):
            self.close_connection = True
            return self.send_json(401, {"success": False, "error": "Authentication required"})
        marker = ACTOR.set(None)
        try:
            ACTOR.set(self.server.worker.identity(self.headers.get("X-Gateway-Credential")))
            path = unquote(urlsplit(self.path).path)
            if self.command == "GET" and path == "/api/esim/access":
                if ACTOR.get() is None:
                    raise OperationError("Worker rental authentication is not configured", 503)
                return self.send_json(200, {"success": True, "rental_enforced": True})
            if self.command == "GET" and path == "/api/esim/batch/events":
                return self.events()
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 <= length <= 8 * 1024 * 1024:
                raise OperationError("Request exceeds 8 MiB", 413)
            raw = self.rfile.read(length)
            if self.command == "POST" and path == "/api/esim/sources/upload":
                message = BytesParser(policy=email_policy).parsebytes(
                    ("Content-Type: " + self.headers.get("Content-Type", "") + "\r\nMIME-Version: 1.0\r\n\r\n").encode() + raw)
                if not message.is_multipart():
                    raise OperationError("Expected a multipart upload", 400)
                sources = []
                for part in message.iter_parts():
                    if part.get_filename():
                        sources.extend(parse_source(Path(part.get_filename()).name, part.get_payload(decode=True) or b"", self.server.worker.settings))
                return self.send_json(200, {"success": True, "data": sources})
            body = json.loads(raw) if raw else {}
            if not isinstance(body, dict):
                raise OperationError("Expected a JSON object", 400)
            data = self.server.worker.dispatch(self.command, path, body)
            self.send_json(200, {"success": True, "data": data})
        except OperationError as error:
            self.send_json(error.status, {"success": False, "error": str(error)})
        except (ValueError, TypeError, KeyError):
            self.send_json(400, {"success": False, "error": "Invalid eSIM request"})
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception:
            self.send_json(500, {"success": False, "error": "Remote eSIM operation failed"})
        finally:
            ACTOR.reset(marker)

    def events(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        try:
            for _ in range(15):
                snapshot = self.server.worker.snapshot()
                with self.server.worker.job_changed:
                    data = json.dumps(snapshot)
                    self.wfile.write(("event: batch\ndata: " + data + "\n\n").encode())
                    self.wfile.flush()
                    self.server.worker.job_changed.wait(timeout=15)
        except (BrokenPipeError, ConnectionResetError, TimeoutError, OperationError):
            pass

    do_GET = handle_request
    do_POST = handle_request


def dispatch(self, method, path, body):
    if method == "GET" and path in ("/api/esim/targets", "/api/esim/ports"):
        return self.inventory()
    if method == "GET" and path.startswith("/api/esim/operations/"):
        return self.operation_status(path.rsplit("/", 1)[1])
    if path == "/api/esim/batch":
        return self.snapshot() if method == "GET" else self.start_batch(body)
    if method == "POST" and path == "/api/esim/batch/cancel":
        snapshot = self.snapshot()
        if snapshot is None:
            raise OperationError("Batch not found", 404)
        permission = {"download": "esim:download", "activate": "esim:enable", "deactivate": "esim:disable", "delete": "esim:delete"}[snapshot["op"]]
        for item in snapshot["items"]:
            self.authorize(self.target(item["com_port"]), permission)
        with self.job_lock:
            if self.job is None or self.job["id"] != snapshot["id"]:
                raise OperationError("Batch changed; refresh before cancelling", 409)
            self.cancel.set()
        return {"cancel_requested": True, "message": "The current card operation will finish before cancellation"}
    if method == "GET" and path == "/api/esim/sources":
        return self.sources()
    pieces = path.split("/")
    if len(pieces) < 5 or pieces[1:3] != ["api", "esim"]:
        raise OperationError("Unknown eSIM route", 404)
    target = self.target(pieces[3])
    action = "/".join(pieces[4:])
    if method == "POST" and action == "session/enter":
        return self.enter(target)
    if method == "POST" and action in ("session/exit", "reset"):
        return self.exit(target)
    reads = {"chip", "profiles", "notifications", "identity"}
    writes = {"profiles/download", "profiles/enable", "profiles/disable", "profiles/delete",
              "profiles/nickname", "notifications/process", "notifications/remove", "msisdn", "provision"}
    if (method == "GET" and action in reads) or (method == "POST" and action in writes):
        return self.operation(target, action, body)
    raise OperationError("Unsupported eSIM operation", 404)


Worker.dispatch = dispatch


def phone_from_text(text):
    pattern = (r"\b(?:msisdn|(?:new\s+)?mobile(?:\s+(?:phone|number))?|phone(?:\s+number)?|telephone(?:\s+number)?)"
               r"\b[ \t]*[:=]?[ \t]*(\+?[0-9](?:[0-9 ()-]*[0-9])?)")
    phones = {re.sub(r"[ ()-]", "", match.group(1)) for match in re.finditer(pattern, text, re.IGNORECASE)}
    phones = {phone for phone in phones if re.fullmatch(r"\+?[0-9]{5,20}", phone)}
    return next(iter(phones)) if len(phones) == 1 else None


def image_phone(image, command):
    from io import BytesIO

    if image.size[0] * image.size[1] > 16000000:
        return None, "image_too_large"
    if not isinstance(command, list) or not command or any(not isinstance(argument, str) or not argument for argument in command):
        raise OperationError("Invalid configured OCR command", 503)
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    try:
        result = subprocess.run([*command, "stdin", "stdout", "-l", "eng", "--psm", "11"],
                                input=buffer.getvalue(), capture_output=True, timeout=15)
    except (OSError, subprocess.TimeoutExpired):
        return None, "unavailable"
    if result.returncode:
        return None, "unavailable"
    phone = phone_from_text(result.stdout.decode("utf-8", errors="replace"))
    return phone, "recognized" if phone else "not_found"


def parse_source(name, raw, settings=None):
    if len(raw) > 4 * 1024 * 1024:
        raise OperationError("Activation source exceeds 4 MiB", 413)
    image_number, image_status = None, "not_found"
    if Path(name).suffix.lower() in (".txt", ".csv"):
        texts = raw.decode("utf-8-sig").splitlines()
    elif Path(name).suffix.lower() in (".png", ".jpg", ".jpeg", ".webp"):
        from io import BytesIO
        from PIL import Image, ImageOps
        import zxingcpp
        with Image.open(BytesIO(raw)) as original:
            image = ImageOps.exif_transpose(original)
            texts = [code.text for code in zxingcpp.read_barcodes(image)]
            if len(texts) == 1:
                image_number, image_status = image_phone(image, (settings or {}).get("ocr_command", ["tesseract"]))
            elif len(texts) > 1:
                image_status = "multiple_codes"
    else:
        return []
    result = []
    for text in texts:
        match = re.search(r"LPA:1\$[^\s,;]+", text)
        if match:
            label = text[:match.start()].strip(" ,;\t")
            parts = match.group().split("$")
            if len(parts) >= 3:
                phone = label if re.fullmatch(r"\+?[0-9]{1,20}", label) else phone_from_text(label)
                result.append({"raw_lpa": match.group(), "activation_code": match.group(),
                               "smdp": parts[1], "matching_id": parts[2], "source": Path(name).name,
                               "label": label, "port_hint": None, "phone_number": phone,
                               "phone_status": "recognized" if phone else image_status})
    if len(result) == 1 and image_number:
        if result[0]["phone_number"] and result[0]["phone_number"] != image_number:
            result[0].update(phone_number=None, phone_status="conflict")
        else:
            result[0].update(phone_number=image_number, phone_status="recognized")
    return result


def operation_deadline(_signal_number, _frame):
    raise OperationError("Card runtime deadline exceeded; inspect operation before retrying", 504)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config")
    parser.add_argument("--card", action="store_true")
    parser.add_argument("--lpac", action="store_true")
    options = parser.parse_args()
    if options.card or options.lpac:
        signal.signal(signal.SIGALRM, operation_deadline)
        signal.alarm(40 if options.card else 250)
    if options.card:
        return card_main()
    if options.lpac:
        return lpac_main()
    if not options.config:
        parser.error("--config is required")
    with open(options.config, "rb") as source:
        config = tomllib.load(source)
    token = os.environ.get("REMOTE_ESIM_WORKER_TOKEN", "")
    if len(token) < 32:
        parser.error("REMOTE_ESIM_WORKER_TOKEN must contain at least 32 characters")
    os.umask(0o077)
    lock_path = config["worker"].get("state_db", "remote-esim.sqlite3") + ".lock"
    ownership = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(ownership, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        parser.error("Another eSIM worker owns this state directory")
    worker = Worker(config)
    host = worker.settings.get("host", "127.0.0.1")
    certificate, key = worker.settings.get("tls_certificate"), worker.settings.get("tls_key")
    if host not in ("127.0.0.1", "localhost") and not (certificate and key):
        parser.error("Non-loopback worker requires TLS certificate and key")
    server = ThreadingHTTPServer((host, worker.settings.get("port", 8091)), Handler)
    server.worker, server.token = worker, token
    if certificate and key:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(certificate, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    print("Remote eSIM worker ready; no card probes run at startup", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()