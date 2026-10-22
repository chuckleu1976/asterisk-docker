import tempfile
import json
import io
import socket
import sqlite3
import ssl
import subprocess
import threading
import unittest
from http.server import ThreadingHTTPServer
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen
import uuid

from remote_esim import ACTOR, Handler, Hardware, OperationError, PublicHTTPSConnection, Target, Worker, card_action, card_main, image_phone, lpac_main, msisdn_record, operation_deadline, parse_source, phone_from_text, pinned_card, public_smdp_addresses, smdp_host


class PublicSmdpTests(unittest.TestCase):
    def test_carrier_hostname_is_normalized_without_an_allowlist(self):
        for value, expected in ((" SMdp.IO. ", "smdp.io"), ("rsp.truphone.com", "rsp.truphone.com"),
                                ("another-carrier.example", "another-carrier.example")):
            self.assertEqual(smdp_host(value), expected)

    def test_hostname_rejects_url_syntax_credentials_and_scoped_addresses(self):
        for value in (None, "", "https://smdp.io", "user@smdp.io", "smdp.io/path", "smdp.io:443",
                      "smdp.io\r\nHost: localhost", "-bad.example", "bad..example", "fe80::1%eth0"):
            with self.subTest(value=value), self.assertRaises(OperationError):
                smdp_host(value)

    def test_public_ipv4_and_ipv6_literals_need_no_dns(self):
        with patch("remote_esim.socket.getaddrinfo") as resolve:
            for address in ("93.184.216.34", "2606:4700:4700::1111"):
                self.assertEqual(public_smdp_addresses(address), [address])
        resolve.assert_not_called()

    def test_nonpublic_literals_are_rejected(self):
        for address in ("127.0.0.1", "10.0.0.1", "172.16.0.1", "192.168.1.1", "169.254.169.254",
                        "100.64.0.1", "0.0.0.0", "224.0.0.1", "192.0.2.1", "::", "::1",
                        "fc00::1", "fe80::1", "ff02::1", "::ffff:127.0.0.1", "64:ff9b::7f00:1"):
            with self.subTest(address=address), self.assertRaisesRegex(OperationError, "public unicast"):
                public_smdp_addresses(address)

    def test_all_dns_answers_must_be_public(self):
        public = (2, 1, 6, "", ("93.184.216.34", 443))
        private = (2, 1, 6, "", ("127.0.0.1", 443))
        for answers in ([private], [public, private], [private, public]):
            with patch("remote_esim.socket.getaddrinfo", return_value=answers), self.assertRaisesRegex(OperationError, "public unicast"):
                public_smdp_addresses("rsp.truphone.com")
        with patch("remote_esim.socket.getaddrinfo", return_value=[public, public]):
            self.assertEqual(public_smdp_addresses("rsp.truphone.com"), ["93.184.216.34"])

    def test_dns_failure_is_explicit(self):
        for response in ([], OSError("DNS failed")):
            options = {"side_effect": response} if isinstance(response, Exception) else {"return_value": response}
            with patch("remote_esim.socket.getaddrinfo", **options), self.assertRaisesRegex(OperationError, "Unable to resolve"):
                public_smdp_addresses("missing.example")

    def test_https_blocks_nonpublic_targets_before_creating_transport(self):
        private = [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("169.254.169.254", 443))]
        for host in ("127.0.0.1", "169.254.169.254", "[::1]", "carrier.example"):
            with patch("remote_esim.socket.getaddrinfo", return_value=private), \
                    patch("remote_esim.build_opener") as opener, self.assertRaisesRegex(OperationError, "public unicast"):
                Hardware({}).https({"url": "https://" + host + "/request"})
            opener.assert_not_called()

    def test_https_pins_dns_preserves_host_and_sni_and_ignores_proxy_environment(self):
        public = [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 443))]
        rebound = [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 443))]
        context = Mock(check_hostname=True, verify_mode=ssl.CERT_REQUIRED)
        encrypted = context.wrap_socket.return_value
        encrypted.makefile.return_value = io.BytesIO(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
        with patch("remote_esim.socket.getaddrinfo", side_effect=[public, rebound]) as resolve, \
                patch("remote_esim.socket.socket") as transport, \
                patch("remote_esim.ssl.create_default_context", return_value=context), \
                patch.dict("os.environ", {"HTTPS_PROXY": "http://127.0.0.1:8081", "ALL_PROXY": "http://127.0.0.1:8082"}):
            result = Hardware({"allowed_smdp_hosts": ["smdp.io"]}).https({
                "url": "https://RSP.TRUPHONE.COM./request?test=1", "tx": "7b7d",
                "headers": ["Content-Type: application/json"]})
        resolve.assert_called_once_with("rsp.truphone.com", 443, type=socket.SOCK_STREAM)
        transport.return_value.connect.assert_called_once_with(("93.184.216.34", 443))
        context.wrap_socket.assert_called_once_with(transport.return_value, server_hostname="rsp.truphone.com")
        transmitted = b"".join(call.args[0] for call in encrypted.sendall.call_args_list)
        self.assertIn(b"Host: rsp.truphone.com\r\n", transmitted)
        self.assertIn(b"POST /request?test=1 HTTP/1.1\r\n", transmitted)
        self.assertEqual(result["payload"], {"rcode": 200, "rx": "7b7d"})

    def test_https_never_follows_carrier_redirect(self):
        context = Mock(check_hostname=True, verify_mode=ssl.CERT_REQUIRED)
        context.wrap_socket.return_value.makefile.return_value = io.BytesIO(
            b"HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n")
        public = [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 443))]
        with patch("remote_esim.socket.getaddrinfo", return_value=public) as resolve, \
                patch("remote_esim.socket.socket") as transport, \
                patch("remote_esim.ssl.create_default_context", return_value=context), \
                self.assertRaisesRegex(OperationError, "redirect refused"):
            Hardware({}).https({"url": "https://rsp.truphone.com/request"})
        self.assertEqual(resolve.call_count, 1)
        transport.return_value.connect.assert_called_once_with(("93.184.216.34", 443))

    def test_connection_falls_back_only_between_pinned_public_addresses(self):
        context = Mock(check_hostname=True, verify_mode=ssl.CERT_REQUIRED)
        first, second = Mock(), Mock()
        first.connect.side_effect = OSError("First address unreachable")
        connection = PublicHTTPSConnection("rsp.truphone.com", addresses=["93.184.216.34", "2606:4700:4700::1111"],
                                           context=context, timeout=25)
        with patch("remote_esim.socket.socket", side_effect=[first, second]) as sockets, \
                patch("remote_esim.socket.getaddrinfo") as resolve:
            connection.connect()
        first.close.assert_called_once()
        second.connect.assert_called_once_with(("2606:4700:4700::1111", 443))
        self.assertEqual(sockets.call_args_list[1].args[0], socket.AF_INET6)
        context.wrap_socket.assert_called_once_with(second, server_hostname="rsp.truphone.com")
        resolve.assert_not_called()

    def test_tls_failure_closes_socket_without_retrying_another_address(self):
        context = Mock(check_hostname=True, verify_mode=ssl.CERT_REQUIRED)
        context.wrap_socket.side_effect = ssl.SSLCertVerificationError("Untrusted issuer")
        connection = PublicHTTPSConnection("rsp.truphone.com", addresses=["93.184.216.34", "93.184.216.35"],
                                           context=context, timeout=25)
        with patch("remote_esim.socket.socket") as transport, self.assertRaises(ssl.SSLCertVerificationError):
            connection.connect()
        transport.assert_called_once()
        transport.return_value.close.assert_called_once()


class RemoteEsimTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.public_addresses = self.enterContext(patch("remote_esim.public_smdp_addresses", return_value=["93.184.216.34"]))
        self.runner = Mock()
        self.worker = Worker({"worker": {"state_db": str(Path(self.directory.name) / "state.sqlite3"), "mutations_enabled": True,
                          "allowed_smdp_hosts": ["example.invalid"]},
                              "targets": [{"id": "slot2", "com_port": "COM3", "bank_id": 1, "slot_nr": 2,
                                           "client_id": 2, "client_slot_nr": 0, "reader": "remote-reader-2",
                                           "expected_eid": "0" * 32, "maintenance": True, "protected": False}]}, self.runner)
        self.addCleanup(self.worker.database.close)
        self.target = self.worker.targets[0]
        self.worker.mappings = Mock(return_value=[{"bank": {"bankId": 1, "slotNr": 2},
                                                  "client": {"clientId": 2, "slotNr": 0}, "state": "ACTIVE"}])

    def test_inventory_never_opens_a_card(self):
        row = self.worker.inventory()[0]
        self.assertEqual(row["slot_label"], "Bank 1 / Slot 2")
        self.assertEqual(row["mapping_label"], "B1:2 -> C2:0")
        self.runner.card.assert_not_called()

    def tenant(self, user="customer-a", permissions=None, rentals=True):
        context = {"user_id": user, "is_admin": False, "rentals": [{"id": "rental-a", "com_port": "COM3",
                   "bank_id": 1, "bank_slot": 2, "client_id": 2, "client_slot": 0,
                   "permissions": permissions or ["esim:read", "esim:enable"]}] if rentals else []}
        identity = {"credential": "synthetic-test-only", "context": context}
        marker = ACTOR.set(identity)
        self.addCleanup(ACTOR.reset, marker)
        self.worker.identity = Mock(return_value=identity)
        self.worker.management = Mock()
        return context

    def test_customer_inventory_matches_asterisk_and_asterisk1_ports(self):
        context = self.tenant()
        context["rentals"][0].update({"com_port": "asterisk", "bank_slot": 0, "client_id": 0, "client_slot": 0})
        self.worker.targets = [Target("bank1-slot0", "asterisk1", 1, 0, 0, 0, "reader")]
        self.worker.mappings = Mock(return_value=[{
            "bank": {"bankId": 1, "slotNr": 0},
            "client": {"clientId": 0, "slotNr": 0},
            "state": "ACTIVE",
        }])
        rows = self.worker.inventory()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["com_port"], "asterisk1")
        self.assertEqual(rows[0]["slot_label"], "Bank 1 / Slot 0")
        self.assertEqual(rows[0]["mapping_label"], "B1:0 -> C0:0")
        self.assertEqual(rows[0]["state"], "ACTIVE")

    def test_customer_inventory_hides_unallocated_targets_and_private_reader(self):
        context = self.tenant()
        self.assertNotIn("reader", self.worker.inventory()[0])
        context["rentals"] = []
        self.assertEqual(self.worker.inventory(), [])
        self.runner.lpac.assert_not_called()

    def test_customer_permission_and_mapping_tuple_are_verified_before_hardware(self):
        context = self.tenant(permissions=["esim:read"])
        with self.assertRaisesRegex(OperationError, "not permitted"):
            self.worker.operation(self.target, "profiles/enable", {"request_id": str(uuid.uuid4())})
        context["rentals"][0]["bank_slot"] = 9
        with self.assertRaisesRegex(OperationError, "not found in active rentals"):
            self.worker.operation(self.target, "profiles", {})
        self.runner.lpac.assert_not_called()

    def test_customer_reserves_reads_and_rechecks_permission_before_each_hardware_command(self):
        context = self.tenant()
        profile = {"iccid": "1234567890123456789", "profileState": "disabled"}
        def revoke_after_read(*args):
            context["rentals"] = []
            return [profile]
        self.runner.lpac.side_effect = revoke_after_read
        with self.assertRaisesRegex(OperationError, "not found in active rentals"):
            self.worker.operation(self.target, "profiles/enable", {"request_id": str(uuid.uuid4()), "iccid": profile["iccid"]})
        self.assertEqual(self.runner.lpac.call_count, 1)
        self.assertTrue(any(call.args[0] == "DELETE" for call in self.worker.management.call_args_list))

    def test_customer_cannot_inspect_other_owner_request_or_global_sources(self):
        self.tenant()
        request_id = str(uuid.uuid4())
        self.worker.database.execute("INSERT INTO operations VALUES (?, 'slot2', 'profiles/enable', 'completed', '{}')", (request_id,))
        self.worker.database.execute("INSERT INTO operation_owners VALUES (?, 'customer-b', 'rental-b')", (request_id,))
        self.worker.database.commit()
        with self.assertRaisesRegex(OperationError, "Unknown operation"):
            self.worker.operation_status(request_id)
        self.assertEqual(self.worker.sources(), [])
        self.worker.job = {"id": "other-customer", "status": "running"}
        self.worker.job_owner = "customer-b"
        self.assertIsNone(self.worker.snapshot())
        with self.assertRaisesRegex(OperationError, "Batch not found"):
            self.worker.dispatch("POST", "/api/esim/batch/cancel", {})
        self.assertFalse(self.worker.cancel.is_set())

    def test_customer_cannot_release_another_rental_session(self):
        self.tenant()
        self.worker.session = self.target.id
        self.worker.session_owner = ("customer-b", "rental-b")
        with self.assertRaisesRegex(OperationError, "Session not found"):
            self.worker.exit(self.target)
        self.assertEqual(self.worker.session, self.target.id)

    def test_legacy_and_reassigned_operation_results_remain_hidden(self):
        context = self.tenant()
        request_id = str(uuid.uuid4())
        self.worker.database.execute("INSERT INTO operations VALUES (?, 'slot2', 'profiles/enable', 'completed', '{}')", (request_id,))
        self.worker.database.commit()
        with self.assertRaisesRegex(OperationError, "Unknown operation"):
            self.worker.operation_status(request_id)
        self.worker.database.execute("INSERT INTO operation_owners VALUES (?, 'customer-a', 'rental-a')", (request_id,))
        self.worker.database.commit()
        self.assertEqual(self.worker.operation_status(request_id)["status"], "completed")
        context["rentals"][0]["id"] = "replacement-rental"
        with self.assertRaisesRegex(OperationError, "Unknown operation"):
            self.worker.operation(self.target, "profiles/enable", {"request_id": request_id})
        self.runner.lpac.assert_not_called()

    def test_provision_requires_enable_and_msisdn_scopes_before_reserving(self):
        context = self.tenant(permissions=["esim:download"])
        body = {"request_id": str(uuid.uuid4()), "activation_code": "LPA:1$example.invalid$synthetic",
                "phone_number": "+123456789"}
        with self.assertRaisesRegex(OperationError, "not permitted"):
            self.worker.operation(self.target, "provision", body)
        context["rentals"][0]["permissions"].append("esim:enable")
        with self.assertRaisesRegex(OperationError, "not permitted"):
            self.worker.operation(self.target, "provision", body)
        self.worker.management.assert_not_called()
        self.runner.lpac.assert_not_called()
        self.runner.card.assert_not_called()

    def test_verified_card_identity_is_reported_with_exact_generation_fence(self):
        self.tenant()
        self.worker.settings["management_api_url"] = "http://authority.invalid/api/v1"
        self.worker.management.side_effect = [{"presence_generation": 5, "modem_generation": 8}, None]
        identity = {"iccid": "1234567890123456789", "imsi": "001010000000001", "msisdn": "00123456789", "msisdn_on_card": "verified"}
        self.runner.card.return_value = dict(identity)
        result = self.worker.runner.card(self.target, "msisdn", {"phone_number": "00123456789"})
        self.assertEqual(result["inventory_sync"], "updated")
        report = self.worker.management.call_args.args
        self.assertEqual(report[:2], ("POST", "/inventory"))
        self.assertEqual(report[2]["bank_slot"], 2)
        self.assertEqual(report[2]["client_id"], 2)
        self.assertEqual(report[2]["presence_generation"], 5)
        self.assertEqual(report[2]["modem_generation"], 8)
        self.assertEqual(report[2]["identity"]["msisdn"], "00123456789")

    def test_inventory_failure_after_verified_write_does_not_repeat_or_fail_the_write(self):
        self.tenant()
        self.worker.settings["management_api_url"] = "http://authority.invalid/api/v1"
        self.worker.management.side_effect = [{"presence_generation": 5, "modem_generation": 8}, OperationError("Unavailable", 503)]
        self.runner.card.return_value = {"iccid": "1234567890123456789", "imsi": "001010000000001",
                                       "msisdn": "00123456789", "msisdn_on_card": "verified"}
        result = self.worker.runner.card(self.target, "msisdn", {})
        self.assertEqual(result["msisdn_on_card"], "verified")
        self.assertEqual(result["inventory_sync"], "failed")
        self.assertIn("do not repeat", result["warning"])
        self.runner.card.assert_called_once()

    def test_batch_results_require_read_permission(self):
        self.tenant(permissions=["esim:download"])
        self.worker.job = {"id": "private-job", "op": "download", "status": "done", "items": []}
        self.worker.job_owner = "customer-a"
        self.worker.job_rentals = {"rental-a"}
        self.assertIsNone(self.worker.snapshot())

    def test_read_only_token_cannot_cancel_an_owned_batch(self):
        context = self.tenant(permissions=["esim:read"])
        self.worker.job = {"id": "private-job", "op": "download", "status": "running", "items": [{"com_port": "COM3"}]}
        self.worker.job_owner = "customer-a"
        self.worker.job_rentals = {"rental-a"}
        self.assertIsNotNone(self.worker.snapshot())
        with self.assertRaises(OperationError) as caught:
            self.worker.dispatch("POST", "/api/esim/batch/cancel", {})
        self.assertEqual(caught.exception.status, 403)
        self.assertFalse(self.worker.cancel.is_set())
        context["rentals"][0]["permissions"].append("esim:download")
        self.assertTrue(self.worker.dispatch("POST", "/api/esim/batch/cancel", {})["cancel_requested"])
        self.assertTrue(self.worker.cancel.is_set())

    def test_cancellation_never_cancels_a_replacement_batch(self):
        self.tenant(permissions=["esim:read", "esim:download"])
        self.worker.job = {"id": "original-job", "op": "download", "status": "running", "items": [{"com_port": "COM3"}]}
        self.worker.job_owner = "customer-a"
        self.worker.job_rentals = {"rental-a"}
        authorize = self.worker.authorize
        def replace_during_authorization(target, permission):
            rental = authorize(target, permission)
            self.worker.job = {**self.worker.job, "id": "replacement-job"}
            return rental
        with patch.object(self.worker, "authorize", side_effect=replace_during_authorization):
            with self.assertRaises(OperationError) as caught:
                self.worker.dispatch("POST", "/api/esim/batch/cancel", {})
        self.assertEqual(caught.exception.status, 409)
        self.assertFalse(self.worker.cancel.is_set())

    def test_http_authority_failure_is_closed_before_inventory_or_card_access(self):
        self.worker.identity = Mock(side_effect=OperationError("Rental authority unavailable", 503))
        server, base = self.start_server()
        request = Request(base + "/api/esim/targets", headers={"Authorization": "Bearer " + server.token,
                                                              "X-Gateway-Credential": "synthetic-credential"})
        with self.assertRaises(HTTPError) as caught:
            urlopen(request, timeout=3)
        self.assertEqual(caught.exception.code, 503)
        caught.exception.close()
        self.worker.mappings.assert_not_called()
        self.runner.lpac.assert_not_called()
        self.runner.card.assert_not_called()

    def test_profile_inventory_uses_lpac_not_pysim(self):
        self.runner.lpac.return_value = [{"iccid": "1234567890123456789", "profileState": "disabled"}]
        result = self.worker.operation(self.target, "profiles", {})
        self.assertEqual(result, self.runner.lpac.return_value)
        self.runner.lpac.assert_called_once_with(self.target, ["profile", "list"], {})
        self.runner.card.assert_not_called()

    def test_profile_state_changes_use_exact_iccid_without_refresh_and_verify(self):
        iccid = "1234567890123456789"
        for command, before, after in (("enable", "disabled", "enabled"), ("disable", "enabled", "disabled")):
            with self.subTest(command=command):
                self.runner.reset_mock()
                self.runner.lpac.side_effect = [[{"iccid": iccid, "profileState": before}], None,
                                                [{"iccid": iccid, "profileState": after}]]
                result = self.worker.operation(self.target, "profiles/" + command,
                                               {"request_id": str(uuid.uuid4()), "iccid": iccid})
                self.assertTrue(result["verified"])
                self.assertEqual(result["state"], after)
                self.assertEqual(self.runner.lpac.call_args_list[1].args[1], ["profile", command, iccid, "0"])
                self.runner.card.assert_not_called()

    def test_profile_state_readback_failure_is_not_silently_accepted(self):
        profile = {"iccid": "1234567890123456789", "profileState": "disabled"}
        self.runner.lpac.side_effect = [[profile], None, [profile]]
        with self.assertRaisesRegex(OperationError, "state readback failed"):
            self.worker.operation(self.target, "profiles/enable", {"request_id": str(uuid.uuid4()), "iccid": profile["iccid"]})
        self.assertEqual(self.runner.lpac.call_count, 3)
        self.runner.card.assert_not_called()

    def test_enabled_profile_cannot_be_deleted(self):
        profile = {"iccid": "1234567890123456789", "profileState": "enabled"}
        self.runner.lpac.return_value = [profile]
        with self.assertRaisesRegex(OperationError, "Disable the profile explicitly"):
            self.worker.operation(self.target, "profiles/delete", {"request_id": str(uuid.uuid4()), "iccid": profile["iccid"]})
        self.runner.lpac.assert_called_once_with(self.target, ["profile", "list"], {})

    def test_profile_delete_verifies_the_iccid_is_absent(self):
        profile = {"iccid": "1234567890123456789", "profileState": "disabled"}
        self.runner.lpac.side_effect = [[profile], None, [profile]]
        with self.assertRaisesRegex(OperationError, "deletion was not verified"):
            self.worker.operation(self.target, "profiles/delete", {"request_id": str(uuid.uuid4()), "iccid": profile["iccid"]})

    def test_nickname_and_clear_nickname_use_lpac_with_readback(self):
        iccid = "1234567890123456789"
        for nickname in ("Test profile", ""):
            with self.subTest(nickname=nickname):
                self.runner.reset_mock()
                self.runner.lpac.side_effect = [[{"iccid": iccid}], None,
                                                [{"iccid": iccid, "profileNickname": nickname or None}]]
                result = self.worker.operation(self.target, "profiles/nickname",
                                               {"request_id": str(uuid.uuid4()), "iccid": iccid, "nickname": nickname})
                self.assertEqual(result["nickname"], nickname)
                self.assertEqual(self.runner.lpac.call_args_list[1].args[1], ["profile", "nickname", iccid, nickname])
                self.runner.card.assert_not_called()

    def test_profile_mutation_rejects_missing_and_unknown_iccid(self):
        for iccid in ("", "1234567890123456789"):
            self.runner.reset_mock()
            self.runner.lpac.return_value = []
            with self.assertRaises(OperationError):
                self.worker.operation(self.target, "profiles/enable", {"request_id": str(uuid.uuid4()), "iccid": iccid})
            self.assertLessEqual(self.runner.lpac.call_count, 1)
            self.runner.card.assert_not_called()

    def test_identity_uses_lpac_inventory_and_pysim_for_card_files(self):
        iccid = "1234567890123456789"
        self.runner.lpac.return_value = [{"iccid": iccid, "profileState": "enabled"}]
        self.runner.card.return_value = {"iccid": iccid, "imsi": "001010000000001"}
        body = {"iccid": iccid}
        self.assertEqual(self.worker.operation(self.target, "identity", body), self.runner.card.return_value)
        self.runner.lpac.assert_called_once_with(self.target, ["profile", "list"], {})
        self.runner.card.assert_called_once_with(self.target, "identity", body)

    def test_disabled_profile_blocks_identity_card_access(self):
        iccid = "1234567890123456789"
        self.runner.lpac.return_value = [{"iccid": iccid, "profileState": "disabled"}]
        with self.assertRaisesRegex(OperationError, "must be enabled"):
            self.worker.operation(self.target, "identity", {"iccid": iccid})
        self.runner.card.assert_not_called()

    def test_notification_removal_uses_lpac_and_readback(self):
        self.runner.lpac.side_effect = [None, []]
        result = self.worker.operation(self.target, "notifications/remove", {"request_id": str(uuid.uuid4()), "seq": "0007"})
        self.assertEqual(result, {"seqNumber": 7, "removed": True})
        self.assertEqual(self.runner.lpac.call_args_list[0].args[1], ["notification", "remove", "7"])
        self.runner.card.assert_not_called()

    def test_pysim_rejects_profile_management_before_loading_card_runtime(self):
        for action in ("chip", "profiles", "profiles/enable", "profiles/disable", "profiles/delete", "profiles/nickname", "notifications"):
            with self.subTest(action=action), self.assertRaisesRegex(OperationError, "only handles identity and MSISDN"):
                card_action(self.target, action, {})

    def test_provision_enables_only_the_downloaded_iccid_then_uses_pysim(self):
        iccid = "1234567890123456789"
        disabled = {"iccid": iccid, "profileState": "disabled"}
        enabled = {"iccid": iccid, "profileState": "enabled"}
        self.runner.lpac.side_effect = [[], None, [disabled], [disabled], None, [enabled], []]
        self.runner.card.side_effect = [{"iccid": iccid, "imsi": "001010000000001"}, {"msisdn": "001234"}]
        result = self.worker.operation(self.target, "provision", {"request_id": str(uuid.uuid4()),
                                       "activation_code": "LPA:1$example.invalid$test-code", "phone_number": "+001234"})
        self.assertTrue(result["downloaded"])
        self.assertTrue(result["enabled"])
        self.assertEqual(result["stages"]["msisdn"], "verified_on_card")
        self.runner.lpac.assert_any_call(self.target, ["profile", "enable", iccid, "0"], {})
        self.assertEqual([entry.args[1] for entry in self.runner.card.call_args_list], ["identity", "msisdn"])

    def test_hardware_command_uses_configured_runtime_without_a_shell(self):
        prefix = ["docker", "exec", "-i", "card-runtime", "python3", "/opt/remote_esim.py"]
        hardware = Hardware({"hardware_command": prefix})
        self.assertEqual(hardware.command("--card"), prefix + ["--card"])
        self.assertEqual(hardware.command("--lpac"), prefix + ["--lpac"])
        self.assertEqual(len(prefix), 6)

    def test_hardware_command_rejects_shell_string(self):
        with self.assertRaises(OperationError):
            Hardware({"hardware_command": "docker exec card-runtime"}).command("--card")

    def test_child_runtime_has_its_own_deadline(self):
        with self.assertRaises(OperationError) as caught:
            operation_deadline(None, None)
        self.assertEqual(caught.exception.status, 504)

    def test_card_runtime_chatter_does_not_corrupt_json_envelope(self):
        def noisy_card(*args):
            print("Card runtime warning")
            return {"verified": True}
        request = {"target": self.target.__dict__, "action": "chip", "body": {}}
        output, errors = io.StringIO(), io.StringIO()
        with patch("remote_esim.sys.stdin", io.StringIO(json.dumps(request))), patch("remote_esim.sys.stdout", output), \
                patch("remote_esim.sys.stderr", errors), patch("remote_esim.card_action", side_effect=noisy_card):
            card_main()
        self.assertTrue(json.loads(output.getvalue())["success"])
        self.assertIn("Card runtime warning", errors.getvalue())

    def test_lpac_runtime_chatter_does_not_corrupt_json_envelope(self):
        def noisy_lpac(*args):
            print("APDU compatibility warning")
            return []
        request = {"target": self.target.__dict__, "settings": {}, "arguments": ["notification", "list"], "body": {}}
        output, errors = io.StringIO(), io.StringIO()
        with patch("remote_esim.sys.stdin", io.StringIO(json.dumps(request))), patch("remote_esim.sys.stdout", output), \
                patch("remote_esim.sys.stderr", errors), patch("remote_esim.pinned_card"), patch("remote_esim.Hardware") as hardware:
            hardware.return_value.lpac_stdio.side_effect = noisy_lpac
            lpac_main()
        self.assertEqual(json.loads(output.getvalue())["data"], [])
        self.assertIn("APDU compatibility warning", errors.getvalue())

    def test_inventory_preserves_raw_backend_identifier_for_dashboard_join(self):
        target = Target(**{**self.target.__dict__, "com_port": "asterisk1"})
        self.worker.targets = [target]
        by_port = {row["com_port"]: row for row in self.worker.inventory()}
        live_sim = {"com_port": "asterisk1"}
        self.assertEqual(by_port[live_sim["com_port"]]["mapping_label"], "B1:2 -> C2:0")
        self.assertNotIn("COM1", by_port)
        self.assertEqual(self.worker.target("asterisk1"), target)
        self.runner.card.assert_not_called()

    def test_com_number_does_not_determine_mapping(self):
        self.worker.mappings.return_value[0]["bank"]["slotNr"] = 7
        row = self.worker.inventory()[0]
        self.assertEqual(row["slot_label"], "Bank 1 / Slot 7")
        self.assertEqual(row["state"], "MISMATCH")
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles", {})
        self.runner.card.assert_not_called()

    def test_mapping_failure_is_unknown_not_unmapped(self):
        self.worker.mappings.side_effect = OSError()
        self.assertEqual(self.worker.inventory()[0]["state"], "UNAVAILABLE")
        self.assertEqual(self.worker.inventory()[0]["slot_label"], "Unknown")

    def test_unmapped_is_explicit(self):
        self.worker.mappings.return_value = []
        self.assertEqual(self.worker.inventory()[0]["state"], "UNMAPPED")

    def test_mutation_requires_idempotency_key(self):
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles/delete", {"iccid": "123"})
        self.runner.card.assert_not_called()

    def test_protected_target_rejects_mutation(self):
        target = Target(**{**self.target.__dict__, "protected": True})
        with self.assertRaises(OperationError):
            self.worker.operation(target, "profiles/delete", {"request_id": str(uuid.uuid4())})
        self.runner.card.assert_not_called()

    def test_success_is_replayed_without_repeating_mutation(self):
        iccid = "1234567890123456789"
        self.runner.lpac.side_effect = [[{"iccid": iccid, "profileState": "disabled"}], {}, []]
        body = {"request_id": str(uuid.uuid4()), "iccid": iccid}
        first = self.worker.operation(self.target, "profiles/delete", body)
        self.assertEqual(first, self.worker.operation(self.target, "profiles/delete", body))
        self.assertEqual(self.runner.lpac.call_count, 3)
        self.runner.card.assert_not_called()

    def test_failed_operation_is_not_automatically_repeated(self):
        self.runner.lpac.side_effect = OperationError("Carrier Refused", 502)
        body = {"request_id": str(uuid.uuid4()), "activation_code": "LPA:1$example.invalid$test-code"}
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles/download", body)
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles/download", body)
        self.assertEqual(self.runner.lpac.call_count, 1)
        self.assertEqual(self.worker.operation_status(body["request_id"])["status"], "failed")
        self.assertEqual(self.worker.operation_status(body["request_id"])["result"], {"error": "Carrier Refused"})

    def test_nonpublic_download_does_not_consume_activation_attempt(self):
        body = {"request_id": str(uuid.uuid4()), "activation_code": "LPA:1$127.0.0.1$test-code"}
        self.public_addresses.side_effect = OperationError("SM-DP+ host must resolve only to public unicast addresses", 403)
        with self.assertRaisesRegex(OperationError, "public unicast"):
            self.worker.operation(self.target, "provision", body)
        self.runner.card.assert_not_called()
        self.runner.lpac.assert_not_called()
        self.assertEqual(self.worker.database.execute("SELECT COUNT(*) FROM download_attempts").fetchone()[0], 0)
        self.assertEqual(self.worker.database.execute("SELECT COUNT(*) FROM operations").fetchone()[0], 0)

    def test_public_carriers_are_not_restricted_by_legacy_allowlist(self):
        self.worker.perform = Mock(return_value={"downloaded": True})
        for host in ("smdp.io", "rsp.truphone.com", "another-carrier.example"):
            result = self.worker.operation(self.target, "profiles/download", {
                "request_id": str(uuid.uuid4()), "activation_code": "LPA:1$" + host + "$test-code"})
            self.assertTrue(result["downloaded"])
            self.public_addresses.assert_called_with(host)
        self.assertEqual(self.worker.perform.call_count, 3)

    def test_batch_accepts_another_public_carrier_before_starting_thread(self):
        with patch("remote_esim.threading.Thread") as thread:
            result = self.worker.start_batch({"op": "download", "items": [
                {"com_port": "COM3", "activation_code": "LPA:1$rsp.truphone.com$test-code"}]})
        self.assertEqual(result["status"], "running")
        self.public_addresses.assert_called_once_with("rsp.truphone.com")
        thread.return_value.start.assert_called_once()
        self.runner.lpac.assert_not_called()

    def test_missing_download_fields_do_not_consume_activation_attempt(self):
        with self.assertRaisesRegex(OperationError, "SM-DP\\+ and matching ID are required"):
            self.worker.operation(self.target, "provision", {"request_id": str(uuid.uuid4())})
        self.runner.card.assert_not_called()
        self.assertEqual(self.worker.database.execute("SELECT COUNT(*) FROM download_attempts").fetchone()[0], 0)

    def test_busy_worker_rejects_second_card_operation(self):
        self.worker.lock.acquire()
        try:
            with self.assertRaises(OperationError):
                self.worker.operation(self.target, "profiles", {})
        finally:
            self.worker.lock.release()
        self.runner.card.assert_not_called()

    def test_default_protection_is_enabled(self):
        target = Target("example", "COM1", 1, 0, 0, 0, "reader")
        self.assertTrue(target.protected)
        self.assertFalse(target.maintenance)

    def test_protected_target_rejects_even_read_only_card_access(self):
        target = Target(**{**self.target.__dict__, "protected": True})
        with self.assertRaises(OperationError):
            self.worker.operation(target, "chip", {})
        self.runner.card.assert_not_called()

    def test_new_manual_request_retries_the_activation_and_preserves_every_attempt(self):
        self.runner.lpac.side_effect = OperationError("Carrier Refused", 502)
        requests = []
        for _ in range(2):
            request_id = str(uuid.uuid4())
            requests.append(request_id)
            with self.assertRaisesRegex(OperationError, "Carrier Refused"):
                self.worker.operation(self.target, "provision", {
                    "request_id": request_id, "activation_code": "LPA:1$example.invalid$not-a-real-code"})
        self.assertEqual(self.runner.lpac.call_count, 2)
        attempts = list(self.worker.database.execute("SELECT fingerprint, operation_id FROM download_attempts"))
        self.assertEqual(len(attempts), 2)
        self.assertEqual(attempts[0][0], attempts[1][0])
        self.assertEqual({row[1] for row in attempts}, set(requests))
        self.assertNotIn("not-a-real-code", str(attempts))
        for request_id in requests:
            self.assertEqual(self.worker.operation_status(request_id)["status"], "failed")

    def test_legacy_activation_ledger_is_migrated_without_losing_history_or_key(self):
        path = str(Path(self.directory.name) / "legacy.sqlite3")
        request_id = str(uuid.uuid4())
        with sqlite3.connect(path) as database:
            self.addCleanup(database.close)
            database.execute("CREATE TABLE download_attempts (fingerprint TEXT PRIMARY KEY, operation_id TEXT)")
            database.execute("INSERT INTO download_attempts VALUES (?, ?)", ("previous-fingerprint", request_id))
            database.execute("CREATE TABLE metadata (key TEXT PRIMARY KEY, value BLOB)")
            database.execute("INSERT INTO metadata VALUES ('fingerprint_key', ?)", (b"existing-key",))
        migrated = Worker({"worker": {"state_db": path}}, self.runner)
        self.addCleanup(migrated.database.close)
        self.assertEqual(migrated.fingerprint_key, b"existing-key")
        self.assertEqual(list(migrated.database.execute("SELECT * FROM download_attempts")), [("previous-fingerprint", request_id)])
        migrated.database.execute("INSERT INTO download_attempts VALUES (?, ?)", ("previous-fingerprint", str(uuid.uuid4())))
        migrated.database.commit()
        reopened = Worker({"worker": {"state_db": path}}, self.runner)
        self.addCleanup(reopened.database.close)
        self.assertEqual(reopened.database.execute("SELECT COUNT(*) FROM download_attempts").fetchone()[0], 2)
        self.assertEqual(reopened.fingerprint_key, b"existing-key")

    def test_installed_profile_is_preserved_when_enable_fails(self):
        profile = {"iccid": "1234567890123456789", "profileState": "disabled"}
        self.runner.lpac.side_effect = [[], {}, [profile], [profile], OperationError("Enable denied"), []]
        request = {"request_id": str(uuid.uuid4()), "activation_code": "LPA:1$example.invalid$test-code"}
        result = self.worker.operation(self.target, "provision", request)
        self.assertTrue(result["downloaded"])
        self.assertTrue(result["partial"])
        self.assertFalse(result["enabled"])
        self.assertEqual(result["iccid"], "1234567890123456789")
        self.assertEqual(self.worker.operation_status(request["request_id"])["result"], result)

    def test_mapping_change_aborts_before_lpac_download(self):
        mapping = self.worker.mappings.return_value
        self.worker.mappings.side_effect = [mapping, []]
        self.runner.lpac.return_value = []
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles/download", {"request_id": str(uuid.uuid4()),
                                                                     "activation_code": "LPA:1$example.invalid$test-code"})
        self.runner.lpac.assert_called_once_with(self.target, ["profile", "list"], {})
        self.runner.card.assert_not_called()

    def test_partial_batch_stops_before_next_target(self):
        self.worker.job = {"status": "running", "items": [{"status": "pending"}, {"status": "pending"}]}
        self.worker.operation = Mock(return_value={"partial": True, "iccid": "1234567890123456789", "warning": "MSISDN denied"})
        body = {"op": "download", "auto_enable": True, "stop_on_error": True,
                "items": [{"com_port": "COM3"}, {"com_port": "COM4"}]}
        self.worker.run_batch(body, [self.target, self.target])
        self.worker.operation.assert_called_once()
        self.assertEqual([item["status"] for item in self.worker.job["items"]], ["failed", "skipped"])

    def test_batch_never_implicitly_replaces_profiles(self):
        with self.assertRaises(OperationError):
            self.worker.start_batch({"op": "download", "replace_existing": True, "items": [{"com_port": "COM3"}]})
        self.runner.card.assert_not_called()

    def test_download_batch_requires_a_code_for_every_target_before_starting(self):
        self.public_addresses.side_effect = OperationError("SM-DP+ host must resolve only to public unicast addresses", 403)
        for item in ({"com_port": "COM3"}, {"com_port": "COM3", "activation_code": "LPA:1$127.0.0.1$test-code"}):
            with self.assertRaises(OperationError):
                self.worker.start_batch({"op": "download", "items": [item]})
        self.assertIsNone(self.worker.job)
        self.runner.card.assert_not_called()
        self.runner.lpac.assert_not_called()

    def test_manual_card_request_cannot_interleave_between_batch_items(self):
        self.worker.job = {"status": "running", "items": []}
        with self.assertRaises(OperationError):
            self.worker.operation(self.target, "profiles", {})
        self.runner.card.assert_not_called()

    def test_source_matches_existing_frontend_schema(self):
        source = parse_source("codes.txt", b"LPA:1$example.invalid$test-code")
        self.assertEqual(source[0]["raw_lpa"], "LPA:1$example.invalid$test-code")
        self.assertEqual(source[0]["smdp"], "example.invalid")
        self.assertEqual(source[0]["matching_id"], "test-code")

    def test_msisdn_text_uses_mobile_label_not_sim_number(self):
        text = "New Mobile Number : 0771234567\nPackage : PRP_16\nSim Number : 940298000012345\n"
        self.assertEqual(phone_from_text(text), "0771234567")
        self.assertIsNone(phone_from_text("Sim Number : 940298000012345"))

    def test_msisdn_text_preserves_international_prefix_and_leading_zeros(self):
        self.assertEqual(phone_from_text("MSISDN: +00 (123) 456-789"), "+00123456789")
        self.assertEqual(phone_from_text("Phone Number = 00123456789"), "00123456789")

    def test_msisdn_text_does_not_choose_between_conflicting_numbers(self):
        self.assertIsNone(phone_from_text("Mobile Number: 0771234567\nPhone Number: 0777654321"))
        self.assertEqual(phone_from_text("MSISDN: 0771234567\nPhone Number: 0771234567"), "0771234567")

    def test_image_ocr_extracts_only_labelled_phone_and_has_a_deadline(self):
        image = Mock(size=(100, 100))
        image.save.side_effect = lambda output, format: output.write(b"test-image")
        reply = Mock(returncode=0, stdout=b"New Mobile Number: 0771234567\nSim Number: 940298000012345")
        with patch("remote_esim.subprocess.run", return_value=reply) as run:
            self.assertEqual(image_phone(image, ["/local/tesseract"]), ("0771234567", "recognized"))
        self.assertEqual(run.call_args.args[0], ["/local/tesseract", "stdin", "stdout", "-l", "eng", "--psm", "11"])
        self.assertEqual(run.call_args.kwargs["timeout"], 15)
        self.assertEqual(run.call_args.kwargs["input"], b"test-image")

    def test_ocr_unavailable_or_timeout_does_not_fail_qr_import(self):
        image = Mock(size=(100, 100))
        for error in (FileNotFoundError(), subprocess.TimeoutExpired("tesseract", 15)):
            with self.subTest(error=type(error).__name__), patch("remote_esim.subprocess.run", side_effect=error):
                self.assertEqual(image_phone(image, ["tesseract"]), (None, "unavailable"))

    def test_oversized_image_is_not_sent_to_ocr(self):
        with patch("remote_esim.subprocess.run") as run:
            self.assertEqual(image_phone(Mock(size=(10000, 10000)), ["tesseract"]), (None, "image_too_large"))
        run.assert_not_called()

    def test_text_source_retains_explicit_phone_metadata(self):
        source = parse_source("codes.txt", b"0771234567 LPA:1$example.invalid$test-code")[0]
        self.assertEqual(source["phone_number"], "0771234567")
        self.assertEqual(source["phone_status"], "recognized")

    def test_image_source_attaches_ocr_msisdn_to_the_single_qr(self):
        barcode = Mock(text="LPA:1$example.invalid$test-code")
        pillow = Mock()
        pillow.Image.open.return_value = MagicMock()
        with patch.dict("sys.modules", {"PIL": pillow, "zxingcpp": Mock(read_barcodes=Mock(return_value=[barcode]))}), \
                patch("remote_esim.image_phone", return_value=("0771234567", "recognized")) as recognize:
            source = parse_source("qr.png", b"test-image", {"ocr_command": ["/private/tesseract"]})[0]
        self.assertEqual(source["phone_number"], "0771234567")
        self.assertEqual(source["phone_status"], "recognized")
        self.assertEqual(recognize.call_args.args[1], ["/private/tesseract"])

    def test_image_with_multiple_qrs_does_not_share_one_printed_msisdn(self):
        barcodes = [Mock(text="LPA:1$example.invalid$first-code"), Mock(text="LPA:1$example.invalid$second-code")]
        pillow = Mock()
        pillow.Image.open.return_value = MagicMock()
        with patch.dict("sys.modules", {"PIL": pillow, "zxingcpp": Mock(read_barcodes=Mock(return_value=barcodes))}), \
                patch("remote_esim.image_phone") as recognize:
            sources = parse_source("qr.png", b"test-image")
        recognize.assert_not_called()
        self.assertEqual(len(sources), 2)
        self.assertTrue(all(source["phone_number"] is None and source["phone_status"] == "multiple_codes" for source in sources))

    def test_image_ocr_failure_preserves_activation_source(self):
        pillow = Mock()
        pillow.Image.open.return_value = MagicMock()
        with patch.dict("sys.modules", {"PIL": pillow, "zxingcpp": Mock(read_barcodes=Mock(return_value=[Mock(text="LPA:1$example.invalid$test-code")]))}), \
                patch("remote_esim.image_phone", return_value=(None, "unavailable")):
            source = parse_source("qr.png", b"test-image")[0]
        self.assertEqual(source["activation_code"], "LPA:1$example.invalid$test-code")
        self.assertEqual(source["phone_status"], "unavailable")

    def test_conflicting_qr_label_and_printed_msisdn_require_manual_review(self):
        pillow = Mock()
        pillow.Image.open.return_value = MagicMock()
        barcode = Mock(text="0777654321 LPA:1$example.invalid$test-code")
        with patch.dict("sys.modules", {"PIL": pillow, "zxingcpp": Mock(read_barcodes=Mock(return_value=[barcode]))}), \
                patch("remote_esim.image_phone", return_value=("0771234567", "recognized")):
            source = parse_source("qr.png", b"test-image")[0]
        self.assertIsNone(source["phone_number"])
        self.assertEqual(source["phone_status"], "conflict")

    def test_duplicate_readers_are_rejected(self):
        config = {"worker": {}, "targets": [self.target.__dict__, {**self.target.__dict__, "id": "other", "com_port": "COM9"}]}
        with self.assertRaises(ValueError):
            Worker(config, self.runner)

    def start_server(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.worker, server.token = self.worker, "test-worker-token-" + "x" * 32
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(thread.join)
        self.addCleanup(server.shutdown)
        return server, f"http://127.0.0.1:{server.server_port}"

    def test_http_requires_worker_bearer_token(self):
        server, base = self.start_server()
        with self.assertRaises(HTTPError) as caught:
            urlopen(base + "/api/esim/targets", timeout=3)
        self.assertEqual(caught.exception.code, 401)
        self.assertEqual(caught.exception.headers["Connection"], "close")
        caught.exception.close()
        request = Request(base + "/api/esim/targets", headers={"Authorization": "Bearer " + server.token, "Connection": "keep-alive"})
        with urlopen(request, timeout=3) as response:
            self.assertEqual(response.headers["Connection"], "close")
            data = json.load(response)
        self.assertTrue(data["success"])
        self.assertEqual(data["data"][0]["mapping_label"], "B1:2 -> C2:0")
        self.runner.card.assert_not_called()

    def test_http_card_route_preserves_existing_response_envelope(self):
        self.runner.lpac.return_value = {"eidValue": self.target.expected_eid}
        server, base = self.start_server()
        request = Request(base + "/api/esim/COM3/chip", headers={"Authorization": "Bearer " + server.token})
        with urlopen(request, timeout=3) as response:
            data = json.load(response)
        self.assertEqual(data["data"]["eidValue"], self.target.expected_eid)

    def test_activation_fingerprint_covers_equivalent_input_forms(self):
        self.runner.lpac.side_effect = OperationError("Carrier Refused", 502)
        for activation in ({"activation_code": "LPA:1$EXAMPLE.invalid$test-code$optional-oid"},
                           {"smdp": "example.invalid", "matching_id": "test-code"}):
            with self.assertRaises(OperationError):
                self.worker.operation(self.target, "provision", {**activation, "request_id": str(uuid.uuid4())})
        self.assertEqual(self.runner.lpac.call_count, 2)

    def test_lpac_stdio_forwards_apdu_and_verified_http_messages(self):
        hardware = Hardware({"lpac_exe": "/not/executed", "allowed_smdp_hosts": ["example.invalid"]})
        hardware.https = Mock(return_value={"type": "http", "payload": {"rcode": 200, "rx": "7b7d"}})
        bridge = Mock(return_value={"type": "apdu", "payload": {"ecode": 0}})
        process = Mock()
        process.stdout = io.StringIO("\n".join(json.dumps(message) for message in [
            {"type": "apdu", "payload": {"func": "connect"}},
            {"type": "http", "payload": {"url": "https://example.invalid/request", "tx": "7b7d"}},
            {"type": "lpa", "payload": {"code": 0, "data": {"installed": True}}}]) + "\n")
        process.wait.return_value = process.poll.return_value = 0
        with patch("remote_esim.subprocess.Popen", return_value=process) as launch:
            result = hardware.lpac_stdio(self.target, ["profile", "download"], {"activation_code": "LPA:1$rsp.truphone.com$test-code"}, bridge)
        self.assertTrue(result["installed"])
        self.assertEqual(launch.call_args.kwargs["env"]["LPAC_APDU"], "stdio")
        self.assertEqual(launch.call_args.args[0][-1], "LPA:1$rsp.truphone.com$test-code")
        bridge.assert_called_once_with({"func": "connect"})
        hardware.https.assert_called_once()
        self.assertEqual(process.stdin.write.call_count, 2)

    def test_lpac_timeout_kills_entire_subprocess_group(self):
        with patch("remote_esim.subprocess.Popen") as launch, patch("remote_esim.os.killpg") as kill:
            process = launch.return_value.__enter__.return_value
            process.pid = 12345
            process.communicate.side_effect = [subprocess.TimeoutExpired("lpac", 270), ("", None)]
            with self.assertRaises(OperationError) as caught:
                Hardware({}).lpac(self.target, ["notification", "list"], {})
            self.assertEqual(caught.exception.status, 504)
            kill.assert_called_once()
            self.assertEqual(kill.call_args.args[0], 12345)

    def test_lpac_labels_carrier_refusal_without_mislabeling_local_or_card_failures(self):
        hardware = Hardware({"lpac_exe": "/not/executed"})
        for stage, detail, expected in (
            ("es9p_authenticate_client", "Refused", "Carrier response (SM-DP+): es9p_authenticate_client: Refused"),
            ("es9p_authenticate_client", "HTTP transport failed", "es9p_authenticate_client: HTTP transport failed"),
            ("es9p_authenticate_client", "Not JSON", "es9p_authenticate_client: Not JSON"),
            ("es10b_authenticate_server", "Refused", "es10b_authenticate_server: Refused"),
        ):
            with self.subTest(stage=stage, detail=detail):
                process = Mock()
                process.stdout = io.StringIO(json.dumps({"type": "lpa", "payload": {"code": -1, "message": stage, "data": detail}}) + "\n")
                process.wait.return_value = process.poll.return_value = 1
                with patch("remote_esim.subprocess.Popen", return_value=process), self.assertRaises(OperationError) as caught:
                    hardware.lpac_stdio(self.target, ["profile", "download"],
                                        {"activation_code": "LPA:1$example.invalid$test-code"}, Mock())
                self.assertEqual(str(caught.exception), expected)

    def test_lpac_https_rejects_unsafe_url_syntax(self):
        hardware = Hardware({"allowed_smdp_hosts": ["example.invalid"]})
        for url in ("http://example.invalid/request", "https://user:password@example.invalid/request",
                    "https://example.invalid:8443/request", "https://example.invalid/request#fragment",
                    "https://example.invalid:invalid/request", "https://example.\ninvalid/request"):
            with self.assertRaises(OperationError) as caught:
                hardware.https({"url": url})
            self.assertEqual(caught.exception.status, 403)

    def test_lpac_https_adds_operator_ca_without_disabling_verification(self):
        context = Mock(check_hostname=True, verify_mode=ssl.CERT_REQUIRED)
        response = MagicMock(code=200)
        response.read.return_value = b"{}"
        opener = Mock()
        opener.open.return_value = response
        hardware = Hardware({"allowed_smdp_hosts": ["example.invalid"], "ca_file": "/approved-ca.pem"})
        with patch("remote_esim.ssl.create_default_context", return_value=context) as create, \
                patch("remote_esim.build_opener", return_value=opener):
            result = hardware.https({"url": "https://example.invalid/request"})
        create.assert_called_once_with()
        context.load_verify_locations.assert_called_once_with(cafile="/approved-ca.pem")
        self.assertTrue(context.check_hostname)
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)
        self.assertEqual(result["payload"]["rcode"], 200)

    def test_lpac_https_certificate_error_is_distinct_from_carrier_refusal(self):
        opener = Mock()
        opener.open.side_effect = URLError(ssl.SSLCertVerificationError("untrusted issuer"))
        with patch("remote_esim.build_opener", return_value=opener):
            with self.assertRaisesRegex(OperationError, "TLS certificate verification failed"):
                Hardware({"allowed_smdp_hosts": ["example.invalid"]}).https({"url": "https://example.invalid/request"})


class PySimAdapterTests(unittest.TestCase):
    def setUp(self):
        try:
            from pySim.ts_51_011 import EF_MSISDN
        except ImportError:
            self.skipTest("Run inside the pinned pySim runtime for codec and APDU tests")
        self.codec = EF_MSISDN()
        self.target = Target("test", "COM3", 1, 2, 2, 0, "exact-reader", "0" * 32, True, False)

    def test_msisdn_codec_preserves_alpha_and_explicit_international_prefix(self):
        original = self.codec.encode_record_hex({"msisdn": "12345"}, 1, total_len=34)
        original = "54657374" + original[8:]
        encoded = msisdn_record(original, 34, "+12345678901")
        decoded = self.codec.decode_record_hex(encoded, 1)
        self.assertEqual(decoded["alpha_id"], "Test")
        self.assertEqual(decoded["dialing_nr"].rstrip("fF"), "12345678901")
        self.assertEqual(decoded["ton_npi"]["type_of_number"], "international")

    def test_msisdn_codec_handles_blank_record_without_guessing_country(self):
        encoded = msisdn_record("ff" * 34, 34, "123456789")
        decoded = self.codec.decode_record_hex(encoded, 1)
        self.assertEqual(decoded["dialing_nr"].rstrip("fF"), "123456789")
        self.assertEqual(decoded["ton_npi"]["type_of_number"], "network_specific")

    def test_identity_reports_msisdn_update_rule_without_writing(self):
        from remote_esim import msisdn_access

        commands = Mock()
        commands.select_file.return_value = ("6200", "9000")
        commands.read_record.return_value = ("80010190008001029700", "9000")
        reference = {"security_attrib_referenced": {"ef_arr_file_id": bytes.fromhex("6f06"), "ef_arr_record_nr": 3}}
        with patch("pySim.ts_102_221.CardProfileUICC.decode_select_response", return_value=reference):
            result = msisdn_access(commands, "6f40")
        self.assertEqual(result, {"update": "never", "arr_file": "6f06", "arr_record": 3})
        commands.read_record.assert_called_once_with("6f06", 3)
        commands.update_record.assert_not_called()
        commands.verify_chv.assert_not_called()

    def test_unreadable_msisdn_access_rule_is_not_treated_as_writable(self):
        from remote_esim import msisdn_access

        commands = Mock()
        with patch("pySim.ts_102_221.CardProfileUICC.decode_select_response", side_effect=ValueError):
            result = msisdn_access(commands, "6f40")
        self.assertEqual(result["update"], "unknown")
        commands.update_record.assert_not_called()

    def test_msisdn_extensions_are_never_overwritten(self):
        with self.assertRaises(OperationError):
            msisdn_record("ff" * 33 + "01", 34, "123456789")

    def test_lpac_uses_one_eid_verified_connection_and_closes_it(self):
        with patch("pySim.transport.pcsc.PcscSimLink") as link, patch("pySim.commands.SimCardCommands") as commands, \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid):
            link.return_value.send_apdu.return_value = ("deadbeef", "9000")
            with pinned_card(self.target) as bridge:
                self.assertFalse(link.return_value.apdu_strict)
                self.assertEqual(bridge({"func": "logic_channel_open", "param": "a0000005591010ffffffff8900000100"})["payload"]["ecode"], 2)
                self.assertEqual(bridge({"func": "transmit", "param": "02ca000000"})["payload"]["data"], "deadbeef9000")
            link.return_value.connect.assert_called_once()
            link.return_value.disconnect.assert_called_once()
            commands.return_value.manage_channel.assert_any_call(mode="close", lchan_nr=2)

    def test_eid_mismatch_prevents_any_lpac_apdu(self):
        with patch("pySim.transport.pcsc.PcscSimLink") as link, patch("pySim.commands.SimCardCommands"), \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value="1" * 32):
            with self.assertRaises(OperationError):
                with pinned_card(self.target):
                    self.fail("EID mismatch must not yield a transport")
            link.return_value.send_apdu.assert_not_called()
            link.return_value.disconnect.assert_called_once()

    def test_active_iccid_mismatch_prevents_msisdn_write(self):
        iccid = "1234567890123456789"
        with patch("pySim.transport.pcsc.PcscSimLink"), patch("pySim.commands.SimCardCommands") as commands, \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid), \
                patch("pySim.utils.dec_iccid", return_value="9876543210987654321"):
            channel = commands.return_value.fork_lchan.return_value
            channel.read_binary.return_value = ("00" * 10, "9000")
            with self.assertRaisesRegex(OperationError, "Active ICCID mismatch"):
                card_action(self.target, "msisdn", {"iccid": iccid, "phone_number": "+123456789"})
            channel.update_record.assert_not_called()

    def test_unsupported_card_instruction_identifies_its_stage_without_writing(self):
        error = RuntimeError("Not a carrier refusal")
        error.sw_actual = "6d00"
        for method, stage in (("select_file", "MF selection"), ("read_binary", "active ICCID read")):
            with self.subTest(stage=stage), patch("pySim.transport.pcsc.PcscSimLink"), \
                    patch("pySim.commands.SimCardCommands") as commands, \
                    patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid):
                channel = commands.return_value.fork_lchan.return_value
                getattr(channel, method).side_effect = error
                with self.assertRaisesRegex(OperationError, "Card instruction not supported during " + stage + r" \(SW=6d00\)"):
                    card_action(self.target, "identity", {"iccid": "1234567890123456789"})
                channel.update_record.assert_not_called()

    def test_msisdn_security_rules_prevent_writes_and_credential_guesses(self):
        iccid = "1234567890123456789"
        for rule in ("ADM1", "PIN2", "never", "unknown", "conditional"):
            with self.subTest(rule=rule), patch("pySim.transport.pcsc.PcscSimLink") as link, \
                    patch("pySim.commands.SimCardCommands") as commands, \
                    patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid), \
                    patch("pySim.utils.dec_iccid", return_value=iccid), \
                    patch("pySim.utils.dec_imsi", return_value="001010000000001"), \
                    patch("remote_esim.msisdn_access", return_value={"update": rule}):
                channel = commands.return_value.fork_lchan.return_value
                channel.read_binary.return_value = ("00" * 10, "9000")
                channel.record_size.return_value = 34
                channel.read_record.return_value = ("ff" * 34, "9000")
                with self.assertRaisesRegex(OperationError, "no card write attempted") as caught:
                    card_action(self.target, "msisdn", {"iccid": iccid, "phone_number": "+123456789"})
                if rule in ("ADM1", "PIN2"):
                    self.assertIn(rule + " authentication", str(caught.exception))
                channel.update_record.assert_not_called()
                channel.verify_chv.assert_not_called()
                commands.return_value.verify_chv.assert_not_called()
                link.return_value.disconnect.assert_called_once()

    def test_msisdn_unrestricted_update_is_verified_by_readback(self):
        iccid = "1234567890123456789"
        phone = "+123456789"
        encoded = msisdn_record("ff" * 34, 34, phone)
        with patch("pySim.transport.pcsc.PcscSimLink"), patch("pySim.commands.SimCardCommands") as commands, \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid), \
                patch("pySim.utils.dec_iccid", return_value=iccid), \
                patch("pySim.utils.dec_imsi", return_value="001010000000001"), \
                patch("remote_esim.msisdn_access", return_value={"update": "always"}):
            channel = commands.return_value.fork_lchan.return_value
            channel.read_binary.return_value = ("00" * 10, "9000")
            channel.record_size.return_value = 34
            records = iter([("ff" * 34, "9000"), (encoded, "9000")])
            channel.read_record.side_effect = lambda path, number: next(records) if path == "6f40" else ("", "9000")
            result = card_action(self.target, "msisdn", {"iccid": iccid, "phone_number": phone})
            self.assertEqual(result["msisdn_on_card"], "verified")
            self.assertEqual(result["msisdn"], phone.lstrip("+"))
            channel.update_record.assert_called_once_with("6f40", 1, encoded, verify=True)

    def test_identity_uses_a_fresh_file_channel_on_the_same_eid_verified_connection(self):
        iccid = "1234567890123456789"
        selections = []
        with patch("pySim.transport.pcsc.PcscSimLink") as link, patch("pySim.commands.SimCardCommands") as commands, \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid), \
                patch("pySim.utils.dec_iccid", return_value=iccid), patch("pySim.utils.dec_imsi", return_value="001010000000001"):
            channel = commands.return_value.fork_lchan.return_value
            channel.select_adf.side_effect = lambda aid: selections.append(("aid", aid.lower()))
            commands.return_value.manage_channel.side_effect = lambda mode, lchan_nr: selections.append((mode, lchan_nr))
            def select_file(fid):
                self.assertEqual(selections[-2:], [("close", 2), ("open", 2)])
                selections.append(("fid", fid))
            channel.select_file.side_effect = select_file
            channel.read_binary.return_value = ("00" * 10, "9000")
            channel.record_size.return_value = 34
            channel.read_record.return_value = ("ff" * 34, "9000")
            result = card_action(self.target, "identity", {"iccid": iccid})
            self.assertEqual(result["iccid"], iccid)
            self.assertEqual(selections[:5], [("open", 2), ("aid", "a0000005591010ffffffff8900000100"),
                                              ("close", 2), ("open", 2), ("fid", "3f00")])
            link.return_value.connect.assert_called_once()
            link.return_value.disconnect.assert_called_once()
            channel.update_record.assert_not_called()

    def test_identity_discovers_the_full_usim_aid_before_reading_imsi(self):
        iccid = "1234567890123456789"
        aid = "a0000000871002ffffffff8907090000"
        directory = "61124f10" + aid
        with patch("pySim.transport.pcsc.PcscSimLink"), patch("pySim.commands.SimCardCommands") as commands, \
                patch("pySim.euicc.CardApplicationISDR.get_eid", return_value=self.target.expected_eid), \
                patch("pySim.utils.dec_iccid", return_value=iccid), patch("pySim.utils.dec_imsi", return_value="001010000000001"):
            channel = commands.return_value.fork_lchan.return_value
            channel.read_binary.return_value = ("00" * 10, "9000")
            channel.record_count.return_value = 1
            channel.record_size.return_value = 34
            channel.read_record.side_effect = lambda path, number: (directory if path == "2f00" else "ff" * 34, "9000")
            result = card_action(self.target, "identity", {"iccid": iccid})
            self.assertEqual(result["iccid"], iccid)
            channel.select_adf.assert_any_call(aid)
            channel.read_binary.assert_any_call("6f07")
            channel.update_record.assert_not_called()


if __name__ == "__main__":
    unittest.main()