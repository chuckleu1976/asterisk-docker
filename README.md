# Introduction
These 2 containers: asterisk-vowifi and pcscd together implement a two-directional SMS- and call-capable VoWiFi-SIP gateway

Based on an excellent work by [VoWiFI work by jolly](https://osmocom.org/projects/foss-ims-client/wiki/VoWiFi_with_Asterisk)
and a few of my patches

WARNING: This isn't production quality and is meant for evaluation only. No warranty or guarantee of any kind. Use at your own risk. I'm not responsible for missed calls, messages, voicemails and so on, any other possible damages and consequences thereof.

Not suitable for emergency calls.

Not suitable as a replacement for a phone system.

License: MIT

# Usage

* Create a `logs` directory near `asterisk-docker` folder. It will contain call and message logs 
* Copy `config/example` configs to `config/1` and modify them
  | Replace                 | With                                                | How to obtain                                                                                  |
  |-------------------------|-----------------------------------------------------|------------------------------------------------------------------------------------------------|
  | `262011503723016`       | Your IMSI                                           | From the SIM card using `pysim-read.py`                                                        |
  | `mnc001.mcc262`         | Your MNC and MCC.                                   | First digits of IMSI. Note the reverse order                                                   |
  | `geheim`                | A new manager password                              | Generate randomly                                                                              |
  | `telegram_token`        | Bot token                                           | Follow [tutorial](https://core.telegram.org/bots/tutorial)                                     |
  | `TELEGRAM_ADDRESS`      | Telegram destination                                |                                                                                                |
  | `YOUR_MSISDN`           | Your phone number                                   |                                                                                                |
  | `2a01:598:408:3003::11` | Correct P-CSCF address                              | Start without it and find a log line `[CFG] received P-CSCF server IP 2a01:598:408:3003::11`   |
  | `YOUR_SMSC`             | Correct SMSC number                                 | From SIM card using `pysim-read.py` with a [patch](https://gerrit.osmocom.org/c/pysim/+/41786) |
  | `SIP_PASSWORD`          | A newly generated password to access SIP endpoint   | Generate randomly                                                                              |
  | `YOUR_DOMAIN`           | An externally-reachable DNS address for your server | Register a (D)DNS domain and point it to the right IP                                          |

* Supply valid `config/1/etc/asterisk/certificate.crt` and `config/1/etc/asterisk/certificate.key` for your domain
* If you're using SELinux, install semodules by running `./semodules_install.sh`
* If you're using user namespaces, change UID in `80-ccid.rules` and install it to `/etc/udev/rules.d/80-ccid.rules`
* Start the container by running `docker-compose up -d` in this directory

# Running several instances

You can run several instances. For this copy directory `config/example` to `config/2`, `config/3`, ... configure each one according to Usage section, copy section in `compose.yaml` and additionally:
* In `epdg.conf` change `local_addrs` to new internal domain name
* Change rtp ports in `rtp.conf` and `compose.yaml`
* Change IMEI in `pjsip.conf`
* Change sip-tls port in `pjsip.conf` and `compose.yaml`
* Change `volumes` subsection in `compose.yaml` to point to a new directory.

# Future work/TODO

* Now `pcscd`, `dbus-daemon`, `polkitd`, `strongswan`, `ami_usim` and `asterisk` run as root-in-container. Check if
we can decrease their privileges to user-in-container
* Switch from custom telegram notification to something like Apprise
