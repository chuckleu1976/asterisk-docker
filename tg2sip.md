# tg2sip integration plan

This document plans how [foobar26/tg2sip](https://github.com/foobar26/tg2sip) sits beside the two running Asterisk instances so that:

- a call that arrives on the VoWiFi line is bridged to a Telegram voice call
- a call that arrives on the gateway's Telegram account is placed out on that same VoWiFi line to a phone number

Nothing in this file is deployed yet. It is the design and the test procedure for that work.

## What tg2sip does

tg2sip is a Docker service that logs in as a **Telegram user account** (not a bot) and speaks SIP with PJSUA2. Audio is PCM between SIP and Telegram. Telegram media is a native P2P voice call (NTgCalls). Signaling uses Pyrogram.

Two directions, from the upstream README:

| Direction | What happens |
|---|---|
| SIP → Telegram | An incoming SIP INVITE is answered and placed as a Telegram call. The dialed user can be `+E.164`, `@username`, a numeric Telegram user id, or the default `TG_FORWARD_USER_ID`. |
| Telegram → SIP | A whitelisted Telegram user calls the gateway account. `telegram.inbound_routes` in `config/config.yaml` maps that caller to a SIP destination. The gateway dials it and bridges audio. |

Hard limits from the upstream project, which this design keeps:

- One Telegram login per gateway process. The gateway account cannot be the same account as the person who receives the forwarded call.
- One call at a time per process. A second call is rejected (`486 Busy Here` on SIP, or a busy decline on Telegram).
- SIP codecs are G.711 (`ulaw`/`alaw`). Telegram is Opus at 48 kHz. Asterisk must transcode to AMR on the IMS leg.

The existing `config/<N>/telegram_token` file is a **bot** token used for text notifications. Bots cannot place Telegram voice calls. tg2sip does not use that file.

## Where calls go today

Each instance is one VoWiFi line. Service `asterisk` is reader 0 (hostname `asterisk1`). Service `asterisk2` is reader 1.

Inside the container, local SIP clients use UDP `5060` (`transport-udp-sip` in the generated `pjsip.conf`). The IMS leg is a separate TCP transport on `ipsec0`. Published host ports are:

| Service | Host SIP | Host RTP | AMI |
|---|---|---|---|
| `asterisk` | `15060/udp` → container `5060/udp` | `10000-10009/udp` | `127.0.0.1:15038` |
| `asterisk2` | `5062/udp` → container `5060/udp` | `10010-10019/udp` | `127.0.0.1:5039` |

`5062` on the host is already Asterisk 2. tg2sip's example config also binds `5062`. Do not run tg2sip on the host network, and do not reuse `5062`.

Dialplan today (`config/example/asterisk/extensions.conf`):

- Inbound IMS calls land in context `volte_ims`, which answers, records, and waits. They are not bridged anywhere.
- Outbound calls from extension `6000` land in `from-sip` and run `Dial(PJSIP/${EXTEN}@volte_ims)`.

## Target layout

Run **two** tg2sip containers, one per Asterisk instance. Each container has its own Telegram user account and its own Pyrogram session. One account cannot be logged in twice, so two live lines need two gateway numbers.

```
Remote phone ──IMS/AMR──► asterisk   ──G.711──► tg2sip1 ──Telegram P2P──► Telegram user
Remote phone ──IMS/AMR──► asterisk2  ──G.711──► tg2sip2 ──Telegram P2P──► Telegram user

Telegram user ──calls gateway 1──► tg2sip1 ──SIP──► asterisk  ──IMS──► dialed number
Telegram user ──calls gateway 2──► tg2sip2 ──SIP──► asterisk2 ──IMS──► dialed number
```

Put `tg2sip1` and `tg2sip2` on the same Compose network as the Asterisk services. They register to the **container** SIP port, not the published host port:

| Gateway | Registers to | SIP user | Own listen port (inside its container) |
|---|---|---|---|
| `tg2sip1` | `asterisk:5060` | `tg2sip` | `5060/udp` |
| `tg2sip2` | `asterisk2:5060` | `tg2sip` | `5060/udp` |

Each gateway only talks to its own Asterisk, so both may use the username `tg2sip` and UDP `5060` inside their own network namespace. RTP between Asterisk and tg2sip stays on the Compose bridge (`direct_media=no`). No host-port publish is required for that path.

`asterisk1` and `asterisk2` can each carry one bridged call at the same time, because each has its own gateway process.

## Asterisk changes

Apply the same pattern to `config/1` and `config/2`. Generated configs that already exist are not rewritten by `generate_config()`, so these edits are made on the live files and then mirrored into `config/example` and `scripts/sim_config_gen.py` so the next bootstrap keeps them.

### PJSIP endpoint

Add a registration endpoint on the UDP transport. Allow only G.711. Asterisk transcodes to AMR for `volte_ims`.

```ini
[tg2sip](endpoint-basic-sip)
transport=transport-udp-sip
context=from-tg2sip
disallow=all
allow=ulaw,alaw
auth=tg2sip-auth
aors=tg2sip
direct_media=no
rtp_symmetric=yes
force_rport=yes
rewrite_contact=yes

[tg2sip-auth](auth-userpass-sip)
username=tg2sip
password=TG2SIP_PASSWORD

[tg2sip](aor-normal-sip)
max_contacts=1
remove_existing=yes
qualify_frequency=60
```

`TG2SIP_PASSWORD` is a long random secret, different for each instance, matching that instance's tg2sip `.env` `SIP_PASSWORD`.

Identify the peer by username/auth, not by a fixed IP. The container address can change.

### Inbound IMS → Telegram

In context `volte_ims`, replace `Answer()` / `Wait(60)` with a `Dial` to the registered gateway. Do not answer the IMS call first. tg2sip sends `180 Ringing` while it sets up Telegram and `200 OK` when the Telegram user answers, and Asterisk should pass that progress through.

Keep the existing `UserEvent`s so sms-gateway still sees the call. `MixMonitor` can stay on the IMS channel.

```ini
[volte_ims]
exten => _.,1,NoOp(Incoming IMS call from ${CALLERID(num)} → Telegram)
 same => n,Set(REC_PATH=/logs/recordings/${UNIQUEID}_${CALLERID(num)}.wav)
 same => n,UserEvent(CallStarted,Direction: inbound,Phone: ${CALLERID(num)},CallId: ${UNIQUEID})
 same => n,MixMonitor(${REC_PATH})
 same => n,Dial(PJSIP/tg2sip,60,g)
 same => n,UserEvent(CallEnded,CallId: ${UNIQUEID},RecordingPath: ${REC_PATH})
 same => n,Hangup()

exten => h,1,UserEvent(CallEnded,CallId: ${UNIQUEID},RecordingPath: ${REC_PATH})
 same => n,Hangup()
```

`Dial(PJSIP/tg2sip)` uses `TG_FORWARD_USER_ID` inside that gateway (one fixed Telegram user per line).

To choose the Telegram destination per call, put the target in the SIP user part. tg2sip treats it as follows:

| Dialed user | Telegram destination |
|---|---|
| `+8613800138000` | that phone number's Telegram account |
| `@alice` | username `alice` |
| `123456789` | numeric user id |
| `tg2sip` (the endpoint name) | `TG_FORWARD_USER_ID` |

Example, still one hop through the local endpoint:

```ini
same => n,Dial(PJSIP/+8613800138000@tg2sip,60,g)
```

The number must be E.164 with a leading `+`. Plain digits are a user id, not a phone number. The destination must already be a Telegram user. The gateway account imports it as a contact.

### Telegram → remote phone

New context `from-tg2sip`. The extension tg2sip dials **is** the phone number to send out over IMS. Reuse the same `Dial(PJSIP/${EXTEN}@volte_ims)` path that extension `6000` already uses.

```ini
[from-tg2sip]
exten => _[+0-9].,1,NoOp(Telegram bridge dialing ${EXTEN} on IMS)
 same => n,Set(REC_PATH=/logs/recordings/${UNIQUEID}_${EXTEN}.wav)
 same => n,UserEvent(CallStarted,Direction: outbound,Phone: ${EXTEN},CallId: ${UNIQUEID})
 same => n,MixMonitor(${REC_PATH})
 same => n,Dial(PJSIP/${EXTEN}@volte_ims,60)
 same => n,UserEvent(CallEnded,CallId: ${UNIQUEID},RecordingPath: ${REC_PATH})
 same => n,Hangup()

exten => h,1,UserEvent(CallEnded,CallId: ${UNIQUEID},RecordingPath: ${REC_PATH})
 same => n,Hangup()
```

On the gateway, `telegram.inbound_routes` is both the route table and the whitelist. Callers who are not listed are declined.

```yaml
telegram:
  inbound_routes:
    "123456789": "+8613800138000"
```

That means: Telegram user id `123456789` calls this gateway account, tg2sip sends `INVITE sip:+8613800138000@asterisk` (or `@asterisk2`), and that Asterisk places the IMS call.

A bare value such as `100` would dial `sip:100@<that asterisk>`, which is not a phone number. Use a full E.164 value with the leading `+` for a remote party.

The gateway Telegram account must accept calls from those users (Telegram → Settings → Privacy → Calls).

## tg2sip configuration

Clone [foobar26/tg2sip](https://github.com/foobar26/tg2sip) next to this repo, for example `../tg2sip`. Do not copy it into the Asterisk image. PJSIP in tg2sip is GPL and ntgcalls is GPLv3; the upstream project expects to run as its own container.

Two env files, one per line. Secrets stay in those files, not in git.

`tg2sip1.env`:

```
TG_API_ID=...
TG_API_HASH=...
SIP_USERNAME=tg2sip
SIP_PASSWORD=<secret shared with config/1>
SIP_DOMAIN=asterisk
SIP_REGISTRAR=asterisk:5060
TG_FORWARD_USER_ID=<numeric id that receives this line's inbound calls>
```

`tg2sip2.env` is the same shape with `SIP_DOMAIN=asterisk2`, `SIP_REGISTRAR=asterisk2:5060`, the instance-2 password, and that line's own Telegram api id, hash, session, and forward target.

`config/config.yaml` for each gateway sets the SIP listen port to `5060` (the container's own port, not the host's `5062`) and the `inbound_routes` map above.

Compose sketch, added beside the existing services so the Docker DNS names `asterisk` and `asterisk2` resolve:

```yaml
  tg2sip1:
    build: ../tg2sip
    env_file: ../tg2sip/tg2sip1.env
    volumes:
      - ../tg2sip/config1:/app/config:ro
      - ../tg2sip/sessions1:/app/sessions
    restart: unless-stopped
    depends_on: [asterisk]

  tg2sip2:
    build: ../tg2sip
    env_file: ../tg2sip/tg2sip2.env
    volumes:
      - ../tg2sip/config2:/app/config:ro
      - ../tg2sip/sessions2:/app/sessions
    restart: unless-stopped
    depends_on: [asterisk2]
```

First login is interactive and must be done once per gateway, before `restart: unless-stopped` is left running:

```bash
docker compose run --rm tg2sip1 python -m src.auth
docker compose run --rm tg2sip2 python -m src.auth
```

Each command writes a `sessions/<name>.session` file. That file is a logged-in Telegram session. Do not commit it.

## Codec and media notes

- `volte_ims` allows only AMR. The tg2sip endpoint allows only `ulaw` and `alaw`. `modules.conf` has `autoload=yes`, so `codec_ulaw` and the AMR codec already used for VoWiFi should both load. Confirm with `core show translation` after the endpoint exists. If `ulaw` has no path to `amr`, the bridge will connect with no audio.
- `direct_media=no` on both the tg2sip endpoint and `volte_ims` keeps both media legs on Asterisk so transcoding can happen.
- Expect roughly 20–40 ms extra delay from the resample, plus Telegram's own jitter buffer.
- Video is out of scope. Leave `VIDEO_SOURCE_URL` empty.

## Web management page

Operators should not edit `tg2sip1.env` or `config.yaml` by hand after the first install. Add one page to the existing sms-gateway Svelte app, next to Phone number, Platform, and Call log on `SimDashboard`.

New page: `sms-gateway/frontend/src/pages/TelegramPage.svelte`, reached as `currentPage === 'telegram'` from `App.svelte`. It lists the two lines that exist today, `asterisk` and `asterisk2`. Each card is one gateway.

### What the page shows

| Block | Content |
|---|---|
| Line | Hostname, MSISDN already known to sms-gateway, gateway container name |
| Telegram session | `not logged in`, `logged in`, or `login needs a code` |
| SIP trunk | `registered` or `down`, from `pjsip show endpoint tg2sip` on that Asterisk |
| Call | `idle` or `busy`. foobar26/tg2sip handles one call per process, so this is a single lamp, not a queue |
| Inbound target | The Telegram user who receives calls that arrive on this SIM. Stored as `TG_FORWARD_USER_ID`, or as `+E.164` / `@username` |
| Outbound routes | Table of who may call this gateway account, and which phone number Asterisk then dials |

The page does not display the Telegram api hash, the SIP password, or the session file.

### What the operator can change

- Inbound target for that line. Saving it rewrites that gateway's forward setting and restarts only that container.
- Outbound routes. One row is a Telegram caller (numeric user id, `@username`, or `+E.164`) and a destination phone number in E.164. Saving rewrites `telegram.inbound_routes` for that gateway. A caller who is not in the table is declined, which is already how tg2sip works.
- Start or stop that gateway container.
- A login action when the session file is missing. The page asks for the gateway phone number, then the code Telegram sends, then the 2FA password if that account has one. Those values are sent once to the gateway's existing `python -m src.auth` flow and are not stored in the sms-gateway database.

First-time api id and api hash stay in the env file on the host. Putting those fields on a page that many operators open is a larger secret than this page needs.

### API

sms-gateway already fronts the other management pages, so the new routes live there and the page calls them through the existing API client. Suggested routes:

| Method | Path | Effect |
|---|---|---|
| `GET` | `/tg2sip` | Both lines: session, SIP registration, idle/busy, forward target, routes |
| `PUT` | `/tg2sip/{instance}/forward` | Set who receives inbound SIM calls |
| `PUT` | `/tg2sip/{instance}/routes` | Replace the outbound route table |
| `POST` | `/tg2sip/{instance}/session` | Submit phone, then code, then optional 2FA password |
| `POST` | `/tg2sip/{instance}/power` | `start` or `stop` the container |

`{instance}` is `1` or `2`. Status reads are Asterisk CLI over the existing AMI/docker path plus `docker inspect` of `tg2sip1` / `tg2sip2`. Writes go to that gateway's config file, then `docker compose restart` of that one service. A write must not restart the other line, and must not restart `pcscd`.

### Page layout

```
Telegram bridge
┌─ asterisk1 · +86… ──────── SIP registered · idle ─┐
│  Inbound calls ring:  [@alice            ] [Save] │
│  Who may call out                                      │
│  123456789     →  +8613800138000          [Remove]    │
│  [Add caller]  [Add number]                [Save]     │
│  [Stop gateway]                                        │
└────────────────────────────────────────────────────────┘
┌─ asterisk2 · +86… ─────── session missing · down ─┐
│  [Log in this Telegram account]                        │
└────────────────────────────────────────────────────────┘
```

The login control opens a short dialog: phone number, then code, then 2FA only if the gateway asks for it. While a call is `busy`, Save and Stop stay disabled so a config restart cannot cut the live bridge.

## Work breakdown

1. Add the `tg2sip` endpoint, auth, and AOR to instance 1 and 2, and to the generator in `scripts/sim_config_gen.py`.
2. Change `volte_ims` to `Dial` the gateway instead of `Wait(60)`. Add context `from-tg2sip`.
3. Add `tg2sip1` and `tg2sip2` to `scripts/sim_config_gen.py` so `compose.yaml` is regenerated with them.
4. Create the two Telegram gateway accounts, api id/hash, sessions, and `inbound_routes`.
5. Add `GET/PUT/POST /tg2sip` on sms-gateway and the `TelegramPage` described above.
6. Reload PJSIP and the dialplan, start the gateways, then run the tests below.

Do not `module reload res_pjsip.so`. Restart the Asterisk container, or use `dialplan reload` only when the change is dialplan-only.

## Test manual

Prerequisites: both Asterisk containers are up, each IMS registration is `Registered`, and IPsec is established (`swanctl --list-sas` shows `ESTABLISHED`). You have two Telegram apps available: the gateway account (or a second phone logged into it is not required after `src.auth`) and a normal user account that will place and receive Telegram calls. You also have a remote mobile phone that can call, and be called by, each SIM's number.

### 1. Gateway login

From the tg2sip checkout, with the Compose project started far enough that `docker compose run` works:

```bash
docker compose run --rm tg2sip1 python -m src.auth
docker compose run --rm tg2sip2 python -m src.auth
```

Enter that gateway's phone number, the login code, and the 2FA password if the account has one. A session file must appear under that gateway's `sessions` directory.

Then:

```bash
docker compose up -d tg2sip1 tg2sip2
docker compose logs -f tg2sip1
```

The log line to look for is a SIP registration status of `200`.

### 2. Asterisk sees the registration

```bash
docker compose exec asterisk asterisk -rx 'pjsip show endpoints'
docker compose exec asterisk2 asterisk -rx 'pjsip show endpoints'
```

Each output must list endpoint `tg2sip` with a contact that is `Avail` or `Reachable`, not `Unavail`. If it is missing:

```bash
docker compose exec asterisk asterisk -rx 'pjsip set logger on'
docker compose logs --tail 50 tg2sip1
```

Check that `SIP_PASSWORD` matches `[tg2sip-auth]`, and that `SIP_REGISTRAR` is `asterisk:5060` or `asterisk2:5060`, not `127.0.0.1` and not host port `15060` or `5062`.

### 3. Remote phone → Telegram

Call the MSISDN of instance 1 from an ordinary phone.

Expected:

1. tg2sip1 log shows an incoming SIP INVITE, then a Telegram `requestCall`.
2. The Telegram user in `TG_FORWARD_USER_ID` (or the `+E.164` / `@user` / id you dialed) gets a Telegram voice call from the **gateway account**, not from the bot token.
3. After you answer on Telegram, both sides have two-way audio.
4. Hangup on either side clears both legs.

Repeat against instance 2's MSISDN and confirm the ringing Telegram account is the one configured for `tg2sip2`, not `tg2sip1`.

While the first call is up, place a second call to the same MSISDN. tg2sip must reject it (`486` in the Asterisk log, or the second Telegram call never connects). The other instance must still be able to take its own call.

### 4. Telegram → remote phone

On the Telegram user listed in `inbound_routes`, call the **gateway** account for instance 1 (the account that `tg2sip1` logged in as).

Expected:

1. tg2sip1 log shows `incoming TG call from user <id>`.
2. Asterisk instance 1 shows an INVITE into context `from-tg2sip` for the E.164 value in that route.
3. The remote phone rings. Caller ID is the SIM's MSISDN.
4. Answer. Audio works in both directions.
5. Hangup on the phone or in Telegram clears both legs.

Repeat with the instance 2 gateway account and confirm the outbound call uses instance 2's SIM.

A Telegram account that is not in `inbound_routes` must be declined. No INVITE should appear in Asterisk.

### 5. Failure checks

| Symptom | What to check |
|---|---|
| Registration never reaches `200` | Password mismatch, wrong registrar host, or Asterisk still reloading. Look at `pjsip set logger on`. |
| IMS call answers and stays silent for 60 s | Dialplan still has `Wait(60)` and was not reloaded. |
| Telegram call is created but one side has no audio | `core show translation` path between `ulaw` and `amr`. Then tg2sip `LOG_LEVEL=DEBUG` and the bridge lines `first SIP→port frame` and `first frame received from ntgcalls`. |
| Call connects, then drops after 10–30 s | Telegram ICE did not complete. Upstream README: look for `updatePhoneCallSignalingData` in the gateway log. |
| Telegram → phone never leaves Asterisk | Route value is not E.164 with `+`, or context `from-tg2sip` is not the endpoint's `context`. |
| `AUTH_KEY_UNREGISTERED` | Delete that gateway's session file and run `python -m src.auth` again. |

### 6. Management page

Open the sms-gateway UI, go to the Telegram bridge page, and confirm both cards match the CLI:

- Instance 1 shows SIP `registered` after section 2, and the forward target you configured.
- Add a route, save, and place the Telegram → phone call from section 4. The dialed number must be the one just saved, not a value left in the yaml from an earlier edit.
- Remove that caller from the table, save, and call again. The gateway must decline the call and Asterisk must show no new INVITE.
- While a call is up, the card shows `busy` and Save is disabled. After hangup it returns to `idle`.

### 7. Stop

```bash
docker compose stop tg2sip1 tg2sip2
```

Stopping the gateways does not unregister the VoWiFi lines. Inbound IMS calls will fail the `Dial` to `tg2sip` until the gateway is started again. Restore the previous `Wait(60)` dialplan first if you need the old auto-answer behaviour while tg2sip is off.
