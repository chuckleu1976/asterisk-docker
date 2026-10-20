# Remote SIM eSIM Worker

This checkout uses the existing eSIM UI with an authenticated Linux worker,
not Windows COM/AT access or a local Windows lpac executable:

```text
Browser -> SMS Gateway -> Linux worker -> remote PC/SC reader -> SIM bank slot
```

The `com_port` configuration value must exactly match the raw `com_port` in
`/api/sims/info`, not the UI display label. This Asterisk checkout uses
`asterisk1`, `asterisk2`, and so on; the frontend displays these as COM1, COM2,
and so on. Configuring `COM1` instead of `asterisk1` prevents the row join and
shows Unknown even when the mapping API works. Never derive bank slots from
either identifier. The dashboard obtains actual slot/mapping labels from the live
remsim management API; unavailable mappings appear as Unknown, not a guessed slot.
Protected targets are listed without opening their cards, even for EID probes.

The existing unauthenticated dashboard uses `/api/sim-mappings`, which returns
only COM, slot label, mapping label, and mapping state. The gateway authenticates
to the worker itself and filters reader/EID fields out of this response.
`/api/esim/*` management routes have no separate browser login in this deployment,
as requested. Browser requests from foreign origins are rejected, but this is
not user authentication: anyone who can directly reach the gateway can operate
an enabled maintenance target. Restrict access to a trusted LAN or an
authenticated reverse proxy. The worker still requires its private bearer token.

## Runtime

Run card commands inside the Linux remsim-client runtime with the shared PC/SC
socket and the matching libpcsclite. A host-side worker can use a fixed
`worker.hardware_command` argument array, for example `docker exec -i` followed
by the runtime name, Python executable, and deployed worker script. It must not
be a shell string. The worker appends `--card` or `--lpac` and sends the request
over stdin. Deploy the helper and dependencies in durable container storage
before recreating that runtime. Child-side deadlines also bound commands if
the host-side container client disconnects. Do not open the physical bank reader:
bankd owns it. Do not restart the shared pcscd namespace for this deployment.

Required components:

- Python 3.11 or newer and the repository's pySim dependencies.
- pySim from pcsc-esim-suite commit `95d3785` or a descendant retaining segmented
  STORE DATA support; use its requirements, including `cmd2>=2.6.2,<3` and the
  Osmocom ASN.1 tools fork. Plain upstream asn1tools is not equivalent.
- lpac 2.3.0 with **both APDU-stdio and HTTP-stdio drivers** enabled. PCSC-only
  lpac builds are insufficient. lpac owns chip information, profile inventory,
  download, enable, disable, delete, nickname, and notification operations.
  pySim owns the exclusive, exact-name PC/SC connection and verifies the pinned
  EID before forwarding lpac APDUs on logical channel 2. Its card-file handler
  only reads ICCID/IMSI and reads or writes EF.MSISDN; it does not manage profiles.
  After verifying EID in ISD-R, the file handler opens a fresh logical channel
  on the same connection for MF/ICCID access and discovers the full USIM AID
  from EF.DIR. It does not issue SIM file commands inside the ISD-R application.
  Profile mutations use exact ICCIDs, readback verification, and refresh flag 0.
  Deletion requires an explicitly disabled profile.
- Optional QR imports: Pillow and zxing-cpp. Text/pasted activation codes do
  not need these packages. Use updated, vetted versions of image dependencies.
- Optional printed-number recognition: Tesseract 5 with English trained data
  on the HTTP worker host, not in the card-command runtime. Set
  `worker.ocr_command` to a fixed argument array; the default is `["tesseract"]`.
  The worker supplies image data on stdin, uses English sparse-text recognition,
  and limits each OCR run to 15 seconds and 16 million image pixels.
- A verified HTTPS trust store. Configure `worker.ca_file` for an operator CA
  when necessary; never disable certificate validation. Public SM-DP+ and
  notification servers are supported without a per-carrier allowlist. Requests
  use HTTPS on port 443; redirects, URL credentials/fragments, and nonpublic
  destinations are rejected.

GSMA-issued carrier TLS certificates may not chain to an ordinary operating-system
trust store. Obtain the required production CA from a verified source; the
[Osmocom eUICC PKI catalogue](https://euicc-manual.osmocom.org/docs/pki/ci/)
publishes live and test bundles separately. Do not enable test roots for live
provisioning. The GSMA RSP2 Root CI1 SHA-256 fingerprint is
`5E3E91FD454327C3AF5D32A7A73BBC59FE43AA7D85FD32D5DB44423F80A56BB3`.
Place the approved bundle at the configured `ca_file` path **inside the hardware
command runtime**, not only on the HTTP worker host. The HTTPS bridge adds it to
system trust while retaining certificate and hostname verification. Keep a
durable host copy and restore it when recreating the container.

The existing Fedora runtime needs its custom PC/SC library directory preserved
in `LD_LIBRARY_PATH` (for example `/opt/pcsc-libs`). A CA-library override must
not accidentally remove that directory.

### Upstream lpac Build

The deployed build comes from [estkme-group/lpac](https://github.com/estkme-group/lpac)
revision `82ada9e53251c7f0cb08bfd9060b17019a30242c`; its runtime version is
`2.3.0.r1.82ada9e53251`. Build with Git, a C compiler, and CMake 3.23 or newer
on a host compatible with the card runtime's libc. This installation uses only
stdio drivers: the Python bridge enforces exact-reader/EID ownership and verified
HTTPS rather than allowing an independent PC/SC or HTTP driver to bypass it.

```bash
git clone https://github.com/estkme-group/lpac.git lpac-src
git -C lpac-src checkout --detach 82ada9e53251c7f0cb08bfd9060b17019a30242c
cmake -S lpac-src -B lpac-build \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/opt/remote-esim/lpac-82ada9e \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DLPAC_WITH_APDU_PCSC=OFF -DLPAC_WITH_HTTP_CURL=OFF \
  -DBUILD_TESTING=OFF
cmake --build lpac-build --parallel 4
DESTDIR="$PWD/lpac-stage" cmake --install lpac-build
```

Copy the staged prefix into the same absolute path in the card runtime and retain
the host artifact for container recreation. Set `worker.lpac_exe` to that prefix's
`bin/lpac` and add its `lib` directory to the hardware command's `LD_LIBRARY_PATH`,
retaining `/opt/pcsc-libs`. Check `lpac version` and `lpac driver list` there before
switching the worker. Neither command accesses a card. Upstream's optional tests
require Criterion, which is not installed in this deployment; they are disabled
in this recipe. Run the worker's regression suite separately as described below.

## Configuration

Start from [the example](../tools/remote_esim.toml.example). All example targets
are protected and mutations are disabled. Confirm each exact PC/SC reader name
and live bank/client tuple, and separately
verify the target EID. Do not commit EIDs, subscriber identifiers, QR codes,
activation tokens, private keys, or bearer credentials.

The former `allowed_smdp_hosts` setting is obsolete and ignored; remove it from
existing configurations. Carrier hostnames are taken from the activation code.
Every resolved address must be public unicast, including IPv6; loopback, private,
link-local, metadata, shared-address, and reserved destinations are blocked.
The HTTPS bridge connects directly to the validated addresses without a second
hostname lookup, while preserving the carrier hostname for TLS SNI/certificate
verification and HTTP Host. Environment proxies are not used. Notification hosts
pass the same checks. Certificate trust is still required for each carrier.

The example mapping URL assumes an SSH tunnel to the management host's loopback
API. Use a trusted tunnel or an authenticated, verified HTTPS endpoint; do not
expose an unauthenticated mapping API to an untrusted network.

Provision the same random bearer token of at least 32 characters in the worker
and gateway's protected environment files. The worker command is:

```bash
python3 tools/remote_esim.py --config /etc/remote-esim/worker.toml
```

The state directory must already exist, be writable only by the service account,
and reside on a local persistent filesystem. It contains verified identity results
and a keyed activation-code fingerprint ledger. Keep it across upgrades/restarts.
An exclusive file lock prevents another worker using that state directory; do
not run independent workers with different state directories against the same
readers. PC/SC connections are exclusive as an additional local guard.

Set the gateway service environment before starting a rebuilt binary:

```text
REMOTE_ESIM_WORKER_URL=http://127.0.0.1:8091
REMOTE_ESIM_WORKER_TOKEN=<the provisioned random bearer token>
```

Loopback HTTP is permitted for a colocated worker or a secured tunnel.
Across hosts use HTTPS; set `worker.tls_certificate` and `worker.tls_key` and
install the signing CA in the gateway's trust store. Non-loopback plaintext
worker listeners and plaintext upstream gateway URLs are rejected. Publish the
browser-facing gateway through HTTPS and restrict network access; the eSIM
proxy does not challenge for a browser username/password.

Build the frontend first, then the Rust release binary (assets are embedded).
Deploy/restart only the gateway/worker in a separately approved maintenance
window. Do not replace the live gateway just to preview the UI.

## Maintenance And Safety

**A worker session/config flag is not a control-plane reservation.** This change
does not add an atomic lease to the management platform's slot-map writers.
Before any card access, an operator must fence those writers and other users of
the target remote reader, remove the target from voice/SMS service, and confirm
that its mapping will not change during maintenance. Keep voice targets protected.
Then set that target's `protected=false`, `maintenance=true`, pinned
`expected_eid`, and, only for approved writes, `worker.mutations_enabled=true`.
Return it to protected/non-maintenance state afterwards.

Live mapping checks run before/after operations and between provisioning stages.
They fail closed on mismatch or an unavailable management API. They are not an
atomic guarantee against an external mapping change midway through an APDU.
Do not enable unattended mutations until management-platform reservation and
identity-cache refresh have been integrated and accepted.

Downloads never implicitly delete/replace an existing profile. A mutation
requires a UUID `request_id`; completed requests replay their recorded result,
while the same failed/interrupted request ID is not automatically retried.
Each new manual Download generates a new request ID and may submit the same
activation code again; an earlier failure is not a permanent local block.
The carrier decides whether the activation is still valid or already used.
The fingerprint ledger retains every attempt and is migrated automatically
from the former one-attempt-per-code schema without deleting history. After a
timeout or interrupted operation, inspect card and carrier state before starting
another manual download. The UI retains the last operation UUID per browser tab
and can inspect its status. No background activation retry is performed.

Provisioning identifies exactly one newly installed ICCID, enables only that
profile, reads identity, and optionally writes EF.MSISDN with readback. A supplied
leading `+` is preserved as international TON; no country code is guessed. No
PIN/ADM guessing, UST changes, global card reset, or destructive replacement is
performed. Partial results retain the downloaded ICCID and stage status.

Verified identity/MSISDN results are returned to the operation UI. They do not
currently update the gateway's cached SIM records, management inventory, or
restart IMS registrations. Refresh those through their owning services before
returning the SIM to production. An EF.MSISDN write does not prove carrier number
assignment. The previously observed carrier `Refused` remains a live activation
blocker, not proof that a QR code expired.

An ES9+ `Refused` result is displayed as `Carrier response (SM-DP+):` followed
by the original lpac stage and detail, for example
`es9p_authenticate_client: Refused`. This identifies the carrier's response,
not a local retry block; transport, response-parsing, and eUICC failures are
not labelled as carrier refusals. New failed operations retain their sanitized
error message in the persistent operation result for later inspection.

Card status failures name the failing step. `SW=6d00` means an unsupported
instruction; `SW=6a82` means an application/file was not found. These are not
carrier responses. A job with verified download/enable stages but failed
identity/MSISDN readback is partial, not a failed installation: inspect the
installed profile and recover the file-access step instead of downloading again.

Batch operations are sequential; cancellation waits for the current operation.
Profile selection must be unambiguous, and replacement is rejected. Job progress
is in memory with SSE snapshots, whereas individual mutation results survive a
worker restart. Card commands are bounded to 45 seconds and lpac operations to
270 seconds, with subprocess-group termination on timeout.

Every download target must have a paired activation source before Start is
enabled. The worker also validates activation fields and public host resolution
before starting a batch, and rechecks the destination before recording an
individual download attempt. Rejected local preflight requests do not open the
card or consume the activation fingerprint. Existing failed/interrupted attempt
records are not cleared by these checks. Each HTTPS request independently
revalidates and pins the destination; a carrier change needs no host-list update.

For an image containing one QR code, local OCR also reads explicitly labelled
mobile/phone/MSISDN text outside the QR. It does not use a SIM number or filename,
infer a country code, or associate one printed number with multiple QR codes.
Leading zeros and an explicit `+` are preserved. Conflicting numbers require
manual review; unavailable OCR does not prevent importing the QR itself.
Each paired source has an editable MSISDN field. Its value takes precedence over
the batch fallback, and invalid nonempty values block Start. Review the detected
number before submitting; recognition alone never writes to the SIM.

## Verification

```bash
python3 -m unittest discover -s tools -p test_remote_esim.py -v
cargo test api::esim::tests
cd frontend && npm run build
```

Run the Python suite inside the pinned pySim runtime as well to execute codec
and APDU tests instead of skipping them. All tests mock card access; no live
download is performed. Accept a live deployment only after a maintenance slot,
valid carrier activation, approved mapping fence, and post-operation inventory
refresh are available.