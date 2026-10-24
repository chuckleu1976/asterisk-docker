import contextlib
import hashlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import Mock, call, patch

from scripts import sim_docker
import start_sim_container_config as sim_cli


ROOT = Path(__file__).resolve().parent.parent
ENTRYPOINT = (ROOT / "asterisk" / "entrypoint.sh").read_text()
RESET_SCRIPT = ENTRYPOINT.split("<<'PYRESET'", 1)[1].split("\n", 1)[1].split("\nPYRESET", 1)[0]


class CardConnectionException(Exception):
    pass


class Reader:
    def __init__(self, name):
        self.name = name
        self.connection = Mock()
        self.createConnection = Mock(return_value=self.connection)

    def __str__(self):
        return self.name


class ReaderResetTests(unittest.TestCase):
    def run_reset(self, hostname, readers):
        modules = {
            "smartcard": types.ModuleType("smartcard"),
            "smartcard.System": types.SimpleNamespace(readers=lambda: readers),
            "smartcard.CardConnection": types.SimpleNamespace(
                CardConnection=types.SimpleNamespace(RESET=1)
            ),
            "smartcard.Exceptions": types.SimpleNamespace(
                CardConnectionException=CardConnectionException
            ),
        }
        output = io.StringIO()
        with patch.dict(sys.modules, modules), patch.object(Path, "read_text", return_value=hostname):
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                exec(compile(RESET_SCRIPT, "entrypoint.sh:PYRESET", "exec"), {})
        return output.getvalue()

    def test_all_128_instances_reset_only_their_exact_reader(self):
        for index in range(128):
            with self.subTest(index=index):
                readers = [Reader(f"osmo-remsim-{i} 00 00") for i in reversed(range(128))]
                output = self.run_reset(f"asterisk{index + 1}", readers)
                self.assertIn("reset OK", output)
                for reader in readers:
                    if reader.name == f"osmo-remsim-{index} 00 00":
                        reader.connection.reconnect.assert_called_once_with(disposition=1)
                        reader.connection.disconnect.assert_called_once_with()
                    else:
                        reader.createConnection.assert_not_called()

    def test_legacy_first_instance_hostname(self):
        reader = Reader("osmo-remsim-0 00 00")
        self.run_reset("asterisk", [reader])
        reader.connection.reconnect.assert_called_once_with(disposition=1)

    def test_invalid_hostname_never_falls_back_to_first_reader(self):
        for hostname in ("other", "asterisk0", "asterisk-1", "asterisk01", "asteriskbad", ""):
            with self.subTest(hostname=hostname):
                reader = Reader("osmo-remsim-0 00 00")
                with self.assertRaisesRegex(ValueError, "Invalid Asterisk hostname"):
                    self.run_reset(hostname, [reader])
                reader.createConnection.assert_not_called()

    def test_missing_reader_reports_warning_without_touching_other_readers(self):
        readers = [Reader("osmo-remsim-10 00 00"), Reader("osmo-remsim-127 00 00")]
        self.assertIn("not found; no reader reset", self.run_reset("asterisk2", readers))
        for reader in readers:
            reader.createConnection.assert_not_called()

    def test_ambiguous_identity_does_not_reset_any_reader(self):
        readers = [Reader("osmo-remsim-1 00 00"), Reader("osmo-remsim-1 00 01")]
        with self.assertRaisesRegex(RuntimeError, "Ambiguous reader identity"):
            self.run_reset("asterisk2", readers)
        for reader in readers:
            reader.createConnection.assert_not_called()

    def test_pcsc_failure_is_reported_and_connection_is_released(self):
        reader = Reader("osmo-remsim-1 00 00")
        reader.connection.reconnect.side_effect = CardConnectionException("card unavailable")
        self.assertIn("reset skipped (card unavailable)", self.run_reset("asterisk2", [reader]))
        reader.connection.disconnect.assert_called_once_with()

    def test_unexpected_failure_is_not_hidden(self):
        reader = Reader("osmo-remsim-1 00 00")
        reader.connection.reconnect.side_effect = RuntimeError("unexpected")
        with self.assertRaisesRegex(RuntimeError, "unexpected"):
            self.run_reset("asterisk2", [reader])
        reader.connection.disconnect.assert_called_once_with()


class ImageVerificationTests(unittest.TestCase):
    def check_image(self, manifest_exists=True, changed=False):
        with tempfile.TemporaryDirectory() as directory:
            module = Path(directory) / "res_pjsip_messaging.so"
            module.write_bytes(b"test messaging module")
            manifest = Path(directory) / "gateway-rpack.sha256"
            if manifest_exists:
                manifest.write_text(f"{hashlib.sha256(module.read_bytes()).hexdigest()}  {module}\n")
            if changed:
                module.write_bytes(b"changed module")
            check = ENTRYPOINT.split("# Graceful shutdown:", 1)[0]
            check = check.replace("/usr/share/asterisk/gateway-rpack.sha256", str(manifest))
            return subprocess.run(["sh", "-c", check], capture_output=True, text=True)

    def test_verified_module_passes(self):
        self.assertEqual(self.check_image().returncode, 0)

    def test_missing_manifest_blocks_startup(self):
        result = self.check_image(manifest_exists=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("verified image-time messaging patch is required", result.stderr)

    def test_changed_module_blocks_startup(self):
        result = self.check_image(changed=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("verified image-time messaging patch is required", result.stderr)


class StartupGateTests(unittest.TestCase):
    def test_wait_retries_until_asterisk_boots(self):
        with patch.object(sim_docker, "docker_compose") as compose, patch.object(sim_docker.time, "sleep") as sleep:
            compose.side_effect = [
                subprocess.CompletedProcess([], 1, "", "not running"),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            sim_docker.wait_for_instance("asterisk128")
        self.assertEqual(compose.call_count, 2)
        self.assertEqual(compose.call_args.args, (
            "exec", "-T", "asterisk128", "asterisk", "-rx", "core waitfullybooted",
        ))
        sleep.assert_called_once()

    def test_wait_is_bounded_and_reports_last_error(self):
        for result, message in (
            (subprocess.CompletedProcess([], 1, "", "not running"), "not running"),
            (subprocess.TimeoutExpired("docker", 10), "boot check timed out"),
        ):
            with self.subTest(message=message):
                with patch.object(sim_docker.time, "monotonic", side_effect=[0, 179, 180, 180]):
                    with patch.object(sim_docker.time, "sleep"), patch.object(sim_docker, "docker_compose") as compose:
                        if isinstance(result, Exception):
                            compose.side_effect = result
                        else:
                            compose.return_value = result
                        with self.assertRaisesRegex(RuntimeError, message):
                            sim_docker.wait_for_instance("asterisk128")
                        self.assertEqual(compose.call_args.kwargs["timeout"], 1)

    def test_start_and_restart_wait_before_returning(self):
        with patch.object(sim_docker, "docker_compose") as compose, patch.object(sim_docker, "_patch_instance") as install:
            compose.return_value = subprocess.CompletedProcess([], 0, "", "")
            with patch.object(sim_docker, "wait_for_instance") as wait:
                operations = Mock()
                operations.attach_mock(compose, "compose")
                operations.attach_mock(install, "install")
                operations.attach_mock(wait, "wait")
                sim_docker.start_instance(128, force_recreate=True)
                sim_docker.restart_instance(1)
        self.assertEqual(operations.mock_calls, [
            call.compose("up", "-d", "--force-recreate", "asterisk128"),
            call.install("asterisk128"),
            call.wait("asterisk128"),
            call.compose("restart", "asterisk"),
            call.wait("asterisk"),
        ])

    def test_failed_start_and_restart_do_not_report_ready(self):
        for operation in (sim_docker.start_instance, sim_docker.restart_instance):
            with self.subTest(operation=operation.__name__):
                with patch.object(sim_docker, "docker_compose") as compose:
                    compose.return_value = subprocess.CompletedProcess([], 1, "", "start failed")
                    with patch.object(sim_docker, "wait_for_instance") as wait:
                        with self.assertRaisesRegex(RuntimeError, "start failed"):
                            operation(2)
                        wait.assert_not_called()

    def test_startup_timeout_propagates_to_caller(self):
        with patch.object(sim_docker, "docker_compose") as compose, patch.object(sim_docker, "_patch_instance"):
            compose.return_value = subprocess.CompletedProcess([], 0, "", "")
            with patch.object(sim_docker, "wait_for_instance", side_effect=RuntimeError("not booted")):
                with self.assertRaisesRegex(RuntimeError, "not booted"):
                    sim_docker.start_instance(128)

    def test_probe_waits_before_settling_and_fails_closed(self):
        slotmap = types.SimpleNamespace(apply_slotmaps=Mock())
        with patch.dict(sys.modules, {"scripts.remsim_slotmap": slotmap}):
            with patch.object(sim_docker, "docker_compose") as compose, patch.object(sim_docker, "wait_for_instance") as wait:
                with patch.object(sim_docker.time, "sleep") as sleep:
                    compose.return_value = subprocess.CompletedProcess([], 0, "", "")
                    sim_docker.start_probe()
                    wait.assert_called_once_with("asterisk")
                    sleep.assert_called_once_with(12)
                    compose.return_value = subprocess.CompletedProcess([], 1, "", "probe failed")
                    wait.reset_mock()
                    sleep.reset_mock()
                    with self.assertRaisesRegex(RuntimeError, "probe failed"):
                        sim_docker.start_probe()
                    wait.assert_not_called()
                    sleep.assert_not_called()

    def test_fallback_probe_waits_and_does_not_return_failed_service(self):
        with patch.object(sim_docker, "docker_compose") as compose, patch.object(sim_docker, "wait_for_instance") as wait:
            compose.side_effect = [
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 0, "", ""),
            ]
            self.assertEqual(sim_docker.get_read_container(), "asterisk")
            wait.assert_called_once_with("asterisk")
            compose.side_effect = [
                subprocess.CompletedProcess([], 0, "", ""),
                subprocess.CompletedProcess([], 1, "", "probe failed"),
            ]
            with self.assertRaisesRegex(RuntimeError, "probe failed"):
                sim_docker.get_read_container()

    def test_auto_mode_stops_after_failed_restart_instead_of_starting_next_line(self):
        sim = {"iccid": "test-card", "imsi": "test-identity", "msisdn": ""}
        with patch.object(sys, "argv", ["start_sim", "--auto", "--restart"]):
            with patch.multiple(
                sim_cli,
                db_init=Mock(),
                db_save_sim=Mock(),
                enumerate_readers_in_container=Mock(return_value=[0, 1]),
                read_sim=Mock(return_value=sim),
                update_instance=Mock(return_value=True),
                restart_instance=Mock(side_effect=RuntimeError("not booted")),
            ):
                with self.assertRaisesRegex(RuntimeError, "not booted"):
                    sim_cli.main()
                sim_cli.restart_instance.assert_called_once_with(1)
                sim_cli.read_sim.assert_called_once_with(0)


if __name__ == "__main__":
    unittest.main()
