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

All Gateway APIs now require a management-backed customer session or API token.
Browser login uses an HttpOnly, SameSite=Strict cookie. LAN HTTP is supported
without a Secure-only cookie or an HTTPS redirect; restrict this deployment to
a trusted LAN because HTTP does not encrypt passwords or tokens in transit.
Carrier SM-DP+ HTTPS, hostname checks, public-address checks, and certificate
validation remain unchanged. The private worker still requires its service token.

## Customer Rentals

Deploy management migration `0011` before enabling Gateway authentication. Set
the same values in the Gateway and HTTP worker service environments:

```ini
MANAGEMENT_API_URL=http://192.168.31.69:8000/api/v1
GATEWAY_ID=asterisk-lan
```

The worker also accepts `worker.management_api_url` and `worker.gateway_id` in
its private TOML. Do not put a user's bearer token in these settings. The Gateway
passes each authenticated caller's credential over the existing private worker
connection. A worker capability check runs before card requests; an older or
unconfigured worker cannot silently bypass rental enforcement. Management or
worker authorization outages deny access, not fall back to anonymous operation.

In management, create an active customer under **Admin / Users**, then open that
customer's detail page and allocate an exact Gateway/raw-port, SIM bank slot,
client slot, expiry, and permissions. Configure the desired and live mapping
before allocating: active rentals fence mapping assignment, release, retry,
ownership transfer, automatic lease release, and controller hardware changes.
COM display labels are not physical slot identifiers. Existing active rentals,
including expired ones, must be explicitly revoked before reassignment. An
in-flight operation delays revocation until its safe stopping point. Operations
reserve the exact resources for a bounded seven-minute lease and renew before
each hardware child command. A failed release after credential revocation leaves
the bounded lease to expire; it does not force an unsafe reassignment.

Customers log in at the normal Gateway URL using their management account.
**My Rentals** shows only current assignments and physical mappings. API tokens
are created/revoked there and shown in full only once; management stores SHA-256
hashes. Tokens inherit neither administrator access nor the ability to mint or
manage other tokens. Creating a token requires a name, not another rental date.
New tokens have `expires_at: null`: permissions are intersected with current
rentals on every request. Expiring or revoking a rental removes access to that
resource; other valid assignments remain usable. With no valid assignments a
token grants no rented-resource access, but the same token automatically works
again after renewal or a new assignment, within its permitted actions.
Revoking the token itself is permanent; rental renewal cannot reactivate it.
Existing fixed-expiry tokens retain their deadlines, shown separately as legacy
token deadlines rather than rental dates. Browser sessions still expire after
12 hours. Suspension and password reset invalidate existing credentials. Queued
batch items and subsequent hardware commands recheck current authority. Existing
streams recheck credentials before sending more events.

Deploy management migration `0012` and the updated management backend before
deploying the Gateway frontend. The migration allows an absent API-token
deadline while still requiring a deadline for browser sessions. The token form
sends only the name and allowed permissions; legacy API clients may still opt
into a separate fixed expiry within 90 days. Migration rollback revokes
rental-bound tokens rather than granting them an arbitrary extended lifetime.

For a Gateway built outside the live checkout, set `ASTERISK_REPO_ROOT` to the
existing absolute stack directory (for this host,
`/home/ht/sourcecode/asterisk-docker`). Both the SIM inventory database and
Telegram control helper resolve through this runtime root. An invalid root
causes an explicit startup error. When unset, the original build-directory
layout remains the default.

Run the token UI tests with
`cd sms-gateway/frontend && node --test src/js/tokens.test.js`, and the backend
tests with `cd server && pytest tests/test_rentals.py tests/test_gateway_credential_migration.py`
in the management checkout. The backend suite uses a disposable PostgreSQL
cluster, not the live management database.

```bash
curl -H 'Authorization: Bearer YOUR_SCOPED_TOKEN' \
  http://192.168.31.25:8080/api/esim/ports
curl -H 'Authorization: Bearer YOUR_SCOPED_TOKEN' \
  http://192.168.31.25:8080/api/esim/asterisk3/profiles
```

Mutations keep the existing API shape and require a fresh UUID `request_id`.
Request IDs, batches, cancellation, sessions, and results are owner-scoped.
Customers cannot scan the administrator's activation-source directory; their
own uploads and pasted codes remain supported. Inventory does not expose private
reader names. Even an administrator needs an active rental for card access;
administrator-wide mapping metadata remains read-only.

### Management Identity Inventory

Verified identity reads and EF.MSISDN write readbacks also update the exact
management SIM slot. Management must have `REMOTE_ESIM_WORKER_TOKEN` configured
with the same private service credential already used by the HTTP worker;
never commit this value. Reports require both that credential and the caller's
active exact rental, and capture card-presence/client snapshot generations
before card access. A changed/removed card or changed client rejects the report.
Operator-entered MSISDN is not overwritten. A reporting failure after a
successful card write returns an `inventory_sync` warning; do not repeat the
write merely to refresh the table.

The bank table is cached inventory, not an automatic APDU scan. A mapped reader
may be marked busy while its last verified identity remains displayed. A blank
EF.MSISDN readback is stored as blank, never replaced by a submitted or printed
phone number. Historical operations without successful identity readback cannot
backfill these fields; use a guarded read-only identity refresh on the active
maintenance mapping. No download, profile toggle, or MSISDN write is needed for
that refresh.

### Configured MSISDN

The Gateway **My Rentals** page and the existing eSIM MSISDN field show
**Configured MSISDN (database)** with a **Save** button and a clear icon. These actions
never open a reader or call the eSIM worker; no maintenance session, activation
code, or ADM credential is needed. The caller must still own a current exact
rental with `esim:msisdn` permission. Preserve an explicit leading `+` and all
digits; no country code is inferred. An empty save clears only the configured
value. Enter in this editor saves configuration, not a profile download.

The value is stored in management `SIMSlot.operator_msisdn`, while
`SIMSlot.sim_msisdn` remains verified card readback. Both values are returned in
the caller's rental context. The management bank table displays the configured
number with a source label, and shows card readback separately when available.
Saving configuration is not proof that the operator assigned the number or
that it was written to the SIM. **Write to SIM** remains a distinct card action.

The Gateway exposes GET/POST
`/api/auth/rentals/{rental_id}/configured-msisdn`. POST forwards to management
`/api/v1/rentals/{rental_id}/configured-msisdn` with `phone_number`,
`presence_generation`, and `identity_generation` from the latest GET. Management
rechecks ownership, current rental/credential scopes, mapping generation, and
card generations under locks; stale-card requests return 409. Audits do not
contain phone numbers. Card inventory updates preserve the configured value.

### MSISDN Card Write Permissions

The rental scope `esim:msisdn` authorizes an application request, not a protected
SIM file update. Identity reads also report `msisdn_access`: the EF.MSISDN
security reference and its decoded UPDATE RECORD rule, when readable. Before
writing, the worker checks this rule and permits an unauthenticated update only
when it explicitly says `always`. Required authentication, forbidden updates,
and unknown rules stop before UPDATE RECORD; no PIN or ADM code is guessed.
Successful writes still require exact readback and management inventory sync.

`SW=6982` means the card's security status was not satisfied. A read-only check
of the current Bank 1 / Slot 2 profile confirmed `ADM1` for updates, referenced
by EF.ARR `6f06`, record 2. A web administrator or checked rental permission
cannot satisfy ADM1. Obtain the profile-specific credential and supported
procedure from the profile issuer; the current worker does not perform ADM
authentication. Do not send credentials in chat, reuse example keys, repeatedly
try codes, or redownload a working profile to resolve this restriction. A blank
EF.MSISDN does not imply the subscription has no operator-assigned phone number.

This rental release scopes eSIM workflows. Global SMS/calls/contacts, recordings,
MMS, platform integrations, and administrative device settings remain
administrator-only. Their historical rows have no tenant owner and must not be
exposed merely by filtering the currently assigned SIM. Customer history and
messaging require an ownership migration before those routes can be enabled.

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
cargo test
cd frontend && npm run build
```

Run the Python suite inside the pinned pySim runtime as well to execute codec
and APDU tests instead of skipping them. All tests mock card access; no live
download is performed. Accept a live deployment only after a maintenance slot,
valid carrier activation, approved mapping fence, and post-operation inventory
refresh are available.

### Rental Verification Cases (2026-10-20)

The expanded suites passed: 219 management tests (including 26 rental cases),
22 Gateway tests, and 94 worker tests with the pinned pySim dependencies and
ResourceWarning treated as an error. Existing dependency deprecation and Rust
unused-code warnings remain; there were no test failures.

| Case | Expected Result | Verification |
| --- | --- | --- |
| Anonymous, wrong password, invalid bearer, legacy Basic | Authentication denied; no card request | Gateway suite and live HTTP checks |
| Two customers list rentals and API-token metadata | Only their own records; another user's token cannot be revoked | Disposable PostgreSQL; live token isolation |
| Non-admin allocation, revocation, resource inventory | Forbidden | Disposable PostgreSQL |
| Same SIM, client slot, or Gateway/raw port allocated twice | Conflict; simultaneous requests have exactly one winner | Disposable PostgreSQL |
| Display COM label or mismatched ReMSIM server | Allocation rejected | Disposable PostgreSQL |
| Future, expired, revoked, or changed-mapping rental | Missing from authorized context; reservation denied | Disposable PostgreSQL |
| Reservation renewal, competing request, incorrect release | Same request renews; competing request/revoke blocked; wrong release does not unlock | Disposable PostgreSQL |
| Unsupported/expired token, scope escalation, token chaining | Denied; admin API token does not inherit browser admin access | Management suite and live HTTP checks |
| Suspension, password/session-version change, logout | Previous credential rejected | Management suite and live HTTP checks |
| Cross-origin or cross-site request | Forbidden | Gateway suite and live HTTP checks |
| Existing admin SSE stream revoked or downgraded | Next event is withheld and the stream closes | Deterministic mocked authority |
| Historical, another user's, or reassigned-rental operation | Result and replay hidden before hardware access | Mocked worker |
| Download/enable/MSISDN combined operation | All required scopes checked before reservation/hardware | Mocked worker |
| Batch results with no read scope; cancellation with read-only scope | Hidden results; cancellation forbidden | Mocked worker; fixes deployed |
| Correctly scoped cancellation; replacement job race | Own job can cancel; replacement job is not cancelled | Mocked worker |
| Authority outage | Service unavailable before mapping/card access | Mocked worker |
| Original three live mappings and protected voice slots | Tuples remain ACTIVE; no-allocation card access denied even for admin | Read-only live metadata |
| Browser invalid login and mobile rendering | Error shown; button recovers; password not stored; branding decoded; no page overflow | Actual HTTP Gateway page at 390px |

Live HTTP tests used two temporary accounts with no physical allocations and
removed their credentials/audit fixtures afterward. Production operations,
activation-attempt records, and fingerprint metadata matched the private
pre-rollout backup after testing. No live download, profile toggle, MSISDN
write, SMS, or call was performed. Real customer card mutations still require
an explicit supervised maintenance test; they are not certified by these mocks.

Customer SMS/call/history workflows remain administrator-only; this verification
does not claim tenant ownership for legacy messaging or call records.