import shutil
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent.parent
CONFIG_DIR = SCRIPT_DIR / "config"
EXAMPLE_DIR = CONFIG_DIR / "example"
COMPOSE_FILE = SCRIPT_DIR / "compose.yaml"


def generate_config(instance, device):
    dest = CONFIG_DIR / str(instance)
    if dest.exists():
        print(f"  config/{instance}/ already exists — skipping generation")
        return

    print(f"  Generating config/{instance}/ from example template...")
    shutil.copytree(str(EXAMPLE_DIR), str(dest))

    imei = device['imei']
    imei_fmt = imei
    if len(imei) >= 15 and '-' not in imei:
        imei_fmt = f"{imei[:8]}-{imei[8:14]}-{imei[14:]}"

    (dest / "ami_usim.ini").write_text(
        "[volte_ims]\n"
        "reader=imsi:000000000000000\n"
        "host=127.0.0.1\n"
        "username=jolly\n"
        "secret=geheim\n"
    )

    hostname = device['hostname']

    (dest / "epdg.conf").write_text(
        "connections {\n"
        "   ims {\n"
        f"      local_addrs  = {hostname}\n"
        "      remote_addrs = epdg.epc.mnc000.mcc000.pub.3gppnetwork.org\n"
        "      vips = ::\n"
        "\n"
        "      local {\n"
        "         auth = eap-aka\n"
        "         id = 0000000000000000@nai.epc.mnc000.mcc000.3gppnetwork.org\n"
        "      }\n"
        "      remote {\n"
        "         id = ims\n"
        "      }\n"
        "      children {\n"
        "         ims {\n"
        "            remote_ts = ::/0\n"
        "            updown = /usr/local/etc/ims.updown\n"
        "            close_action = start\n"
        "            if_id_in = 23\n"
        "            if_id_out = 23\n"
        "         }\n"
        "      }\n"
        "      version = 2\n"
        "   }\n"
        "}\n"
    )

    (dest / "asterisk" / "pjsip.conf").write_text(
        "[global]\n"
        "type=global\n"
        "allow_sending_180_after_183=yes\n"
        "\n"
        "[system]\n"
        "type=system\n"
        "timer_t1=2000\n"
        "\n"
        "; ====== SIP listener\n"
        "[transport-udp-sip]\n"
        "type=transport\n"
        "protocol=udp\n"
        "bind=0.0.0.0:5060\n"
        "local_net=172.18.0.0/16\n"
        "\n"
        "\n"
        ";===============ENDPOINT TEMPLATES\n"
        "\n"
        "[endpoint-basic-sip](!)\n"
        "type=endpoint\n"
        "allow=all\n"
        "media_encryption=no\n"
        "rtp_symmetric=yes\n"
        "force_rport=yes\n"
        "rewrite_contact=yes\n"
        "\n"
        "[auth-userpass-sip](!)\n"
        "type=auth\n"
        "auth_type=userpass\n"
        "\n"
        "[aor-normal-sip](!)\n"
        "type=aor\n"
        "max_contacts=3\n"
        "\n"
        ";===============EXTENSION 6000\n"
        "\n"
        "[6000](endpoint-basic-sip)\n"
        "auth=auth-sip\n"
        "aors=6000\n"
        "callerid=undefined <+000>\n"
        "context=from-sip\n"
        "message_context = msg-from-sip\n"
        "\n"
        "[auth-sip](auth-userpass-sip)\n"
        "password=123456\n"
        "username=6000\n"
        "\n"
        "\n"
        "[6000](aor-normal-sip)\n"
        "\n"
        ";===============VoLTE\n"
        "\n"
        "[volte_ims]\n"
        "type=transport\n"
        "protocol=tcp\n"
        "bind=[::]:5060\n"
        "bind_interface=ipsec0\n"
        "sec_port_c_min=40000\n"
        "sec_port_c_max=44999\n"
        "sec_port_s_min=50000\n"
        "sec_port_s_max=54999\n"
        "p_access_network_info=IEEE-802.11\\;i-wlan-node-id=mywifi\n"
        "sec_encryption=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=registration\n"
        "transport=volte_ims\n"
        "outbound_auth=volte_ims\n"
        f"imei={imei_fmt}\n"
        "server_uri=sip:ims.mnc000.mcc000.3gppnetwork.org\n"
        "client_uri=sip:000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "retry_interval=30\n"
        "fatal_retry_interval=30\n"
        "max_retries=999999999\n"
        "expiration=600000\n"
        "volte=yes\n"
        "manual_register=yes\n"
        "endpoint=volte_ims\n"
        "receive_sms=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=endpoint\n"
        "user_eq_phone=on\n"
        "transport=volte_ims\n"
        "context=volte_ims\n"
        "message_context=volte_ims_msg\n"
        "disallow=all\n"
        "allow=amr\n"
        "bw_value=41\n"
        "outbound_auth=volte_ims\n"
        "aors=volte_ims\n"
        "rewrite_contact=yes\n"
        "from_domain=ims.mnc000.mcc000.3gppnetwork.org\n"
        "from_user=+000\n"
        "volte=yes\n"
        "dedicated_bearer_up=yes\n"
        "100rel=peer_supported\n"
        "moh_passthrough=yes\n"
        "direct_media=no\n"
        "smsc_uri=sip:+000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "\n"
        "[volte_ims]\n"
        "type=auth\n"
        "auth_type=ims_aka\n"
        "username=000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "usim_ami=yes\n"
        "\n"
        "[volte_ims]\n"
        "type=aor\n"
        "contact=sip:000000000000000@ims.mnc000.mcc000.3gppnetwork.org\n"
        "max_contacts=1\n"
        "\n"
        "[volte_ims]\n"
        "type=identify\n"
        "endpoint=volte_ims\n"
        "match=::1\n"
        "\n"
        "[ims.mnc000.mcc000.3gppnetwork.org]\n"
        "type=resolve\n"
        "ip=::1\n"
        "transport=volte_ims\n"
        "\n"
        "[smsoip.ims.mnc000.mcc000.3gppnetwork.org]\n"
        "type=resolve\n"
        "ip=::1\n"
        "transport=volte_ims\n"
    )

    rtp_start = 10000 + (instance - 1) * 10
    rtp_end = rtp_start + 9
    (dest / "asterisk" / "rtp.conf").write_text(
        f"[general]\nrtpstart={rtp_start}\nrtpend={rtp_end}\n"
    )

    tok = dest / "telegram_token"
    if tok.read_text().strip() == "telegram_token":
        tok.write_text("telegram_token\n")

    print(f"  config/{instance}/ created  (IMEI={imei_fmt}, RTP={rtp_start}-{rtp_end})")


def generate_compose(devices):
    lines = [
        "services:",
        "  pcscd:",
        "    build:",
        "      context: ./pcscd",
        "      dockerfile: Dockerfile",
        "    image: ghcr.io/chuckleu1976/pcscd-sysmocom:latest",
        "    restart: always",
        "    privileged: true",
        "    environment:",
        "      - LD_LIBRARY_PATH=/usr/lib64/remsim-libs",
        "      - SIM_MODE=${SIM_MODE:-local}",
        "    volumes:",
        "      - ./pcscd/entrypoint.sh:/entrypoint.sh:ro",
        "      - pcsc-sock:/run/pcscd",
        "      - ./remsim/libs:/usr/lib64/remsim-libs:ro",
        "      - ./remsim/serial:/usr/lib64/pcsc/drivers/serial:ro",
        "      - ./remsim/reader.conf.d:/etc/reader.conf.d:ro",
        "    tmpfs:",
        "      - /run",
        "    devices:",
        "      - /dev/bus/usb:/dev/bus/usb",
    ]

    for dev in devices:
        idx = dev['reader']
        instance = idx + 1
        hostname = dev['hostname']
        svc = "asterisk" if instance == 1 else f"asterisk{instance}"
        sip_port = 5060 + (instance - 1) * 2
        rtp_s = 10000 + (instance - 1) * 10
        rtp_e = rtp_s + 9
        ami_port = 5038 + (instance - 1)

        lines += [
            f"  {svc}:",
            f"    image: phcodercat/asterisk-vowifi:latest",
            f"    hostname: {hostname}",
            f"    build:",
            f"      context: ./asterisk",
            f"      dockerfile: Dockerfile",
            f"      network: host",
            f"    cap_add:",
            f"      - CAP_NET_ADMIN",
            f"    privileged: true",
            f"    devices:",
            f"      - /dev/net/tun",
            f"      - /dev/bus/usb:/dev/bus/usb",
            f"    ports:",
            f"      - {sip_port}:5060/udp",
            f"      - {rtp_s}-{rtp_e}:{rtp_s}-{rtp_e}/udp",
            f"      - 127.0.0.1:{ami_port}:5038/tcp",
            f"    tmpfs:",
            f"      - /run",
            f"    environment:",
            f"      - LD_LIBRARY_PATH=/opt/pcsc-libs",
            f"    volumes:",
            f"      - ./pcsc-libs:/opt/pcsc-libs:ro",
            f"      - ./config/{instance}/epdg.conf:/usr/local/etc/swanctl/conf.d/epdg.conf:Z,ro",
            f"      - ./config/{instance}/asterisk:/etc/asterisk:Z",
            f"      - ./config/{instance}/ami_usim.ini:/usr/local/etc/ami_usim.ini:Z,ro",
            f"      - ./config/{instance}/telegram_token:/usr/local/etc/telegram_token:Z,ro",
            f"      - ../logs/{instance}:/logs:Z",
            f"      - ./sim_inventory.db:/data/sim_inventory.db:Z",
            f"      - pcsc-sock:/run/pcscd",
            f"    pid: service:pcscd",
            f"    depends_on: [pcscd]",
            f"    healthcheck:",
            f"      test: [\"CMD-SHELL\", \"swanctl --list-sas 2>/dev/null | grep -q ESTABLISHED || exit 1\"]",
            f"      interval: 30s",
            f"      timeout: 10s",
            f"      retries: 6",
            f"      start_period: 120s",
            f"    restart: always",
        ]

    lines += ["", "volumes:", "  pcsc-sock:", ""]
    COMPOSE_FILE.write_text('\n'.join(lines))
    print(f"  compose.yaml written ({len(devices)} asterisk service(s))")
