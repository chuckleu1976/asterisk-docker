#!/bin/sh

set -e

if ! test -f /usr/share/asterisk/gateway-rpack.sha256 || ! sha256sum --status -c /usr/share/asterisk/gateway-rpack.sha256; then
    echo "A verified image-time messaging patch is required; build asterisk/Dockerfile.patch before starting this line." >&2
    exit 1
fi

# Graceful shutdown: kill all background children (charon, starter, ami_usim,
# IKE watchdog) so that a container restart/replace does not leave orphaned
# processes in the shared pcscd PID namespace.
LOOP_PID=""
AMI_PID=""
ASTERISK_PID=""

cleanup() {
    echo "Shutting down container processes..."
    if [ -n "$LOOP_PID" ]; then
        kill -TERM "$LOOP_PID" 2>/dev/null || true
        sleep 1
    fi
    if [ -n "$AMI_PID" ]; then
        kill -TERM "$AMI_PID" 2>/dev/null || true
        wait "$AMI_PID" 2>/dev/null || true
    fi
    ipsec stop 2>/dev/null || true
    if [ -n "$ASTERISK_PID" ]; then
        kill -TERM "$ASTERISK_PID" 2>/dev/null || true
        wait "$ASTERISK_PID" 2>/dev/null || true
    fi
    exit 0
}

trap cleanup TERM INT

sleep 3

# Configure charon filelog at the correct strongSwan config path.
# This captures [CFG] received P-CSCF server IP lines to the file so
# ims.updown can extract the P-CSCF IP without relying on swanctl --log.
mkdir -p /usr/local/etc/strongswan.d
cat > /usr/local/etc/strongswan.d/charon-pcscf-log.conf << 'LOGEOF'
charon {
    filelog {
        pcscf {
            path = /var/log/strongswan-pcscf.log
            cfg = 1
            default = -1
            append = yes
            flush_line = yes
        }
    }
}
LOGEOF

# The asterisk containers share the PID namespace with pcscd. A container
# restart reuses the same network namespace but does NOT reap strongSwan
# processes from the previous run, so multiple charon/starter instances end
# up competing for UDP 500/4500. Kill any charon/starter that belongs to
# this container's network namespace, then flush stale xfrm state.
echo "Cleaning up stale processes in this network namespace..."
MY_NET=$(readlink /proc/self/ns/net)

# Kill stale charon/starter (process name matches)
for name in charon starter; do
    for pid in $(pgrep -x "$name" 2>/dev/null || true); do
        PID_NET=$(readlink /proc/$pid/ns/net 2>/dev/null || true)
        if [ "$PID_NET" = "$MY_NET" ]; then
            echo "  Killing stale $name pid=$pid"
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
done

# Kill stale ami_usim.py helpers (process name is python3, match by cmdline)
for pid in $(pgrep -f '/usr/local/bin/ami_usim.py' 2>/dev/null || true); do
    PID_NET=$(readlink /proc/$pid/ns/net 2>/dev/null || true)
    if [ "$PID_NET" = "$MY_NET" ]; then
        echo "  Killing stale ami_usim.py pid=$pid"
        kill -9 "$pid" 2>/dev/null || true
    fi
done

ip xfrm state flush 2>/dev/null || true
ip xfrm policy flush 2>/dev/null || true

ipsec start
sleep 2
swanctl --load-creds && swanctl --load-conns

# Power-cycle only this container's SIM before IKE initiation.
# Resetting every PC/SC reader also interrupts sibling asterisk instances
# that share the pcscd PID namespace and can drop their IMS registration.
echo "Resetting this instance's SIM before IKE initiation..."
python3 - <<'PYRESET' || echo "Warning: SIM reset failed; continuing without a reset" >&2
import pathlib
import re
import sys
from smartcard.System import readers
from smartcard.CardConnection import CardConnection
from smartcard.Exceptions import CardConnectionException

host = pathlib.Path("/etc/hostname").read_text().strip()
match = re.fullmatch(r"asterisk([1-9][0-9]*)?", host)
if match is None:
    raise ValueError("Invalid Asterisk hostname; refusing to select a reader")
idx = int(match.group(1) or "1") - 1
target = f"osmo-remsim-{idx}"

def matches_reader(reader_name, target_name):
    return reader_name.split(maxsplit=1)[:1] == [target_name]

selected = [(i, r) for i, r in enumerate(readers()) if matches_reader(str(r), target)]
if len(selected) > 1:
    raise RuntimeError("Ambiguous reader identity; refusing to reset multiple readers")
if not selected:
    print(f"  Warning: {target} not found; no reader reset", file=sys.stderr)
for i, r in selected:
    c = r.createConnection()
    try:
        c.connect()
        c.reconnect(disposition=CardConnection.RESET)
        print(f"  P{i}: reset OK ({r.name})")
    except CardConnectionException as e:
        print(f"  P{i}: reset skipped ({e})", file=sys.stderr)
    finally:
        c.disconnect()
PYRESET

# Initiate IKE in background — ims.updown writes /tmp/pcscf_ip when ready.
rm -f /tmp/pcscf_ip
swanctl --initiate --child ims &

# Wait for ims.updown to signal P-CSCF is ready via /tmp/pcscf_ip file.
# This is written by ims.updown after it receives the P-CSCF from strongSwan.
echo "Waiting for P-CSCF IP to be set in pjsip.conf..."
WAIT=0
while [ ! -f /tmp/pcscf_ip ] && [ $WAIT -lt 120 ]; do
    sleep 3
    WAIT=$((WAIT + 3))
done
if [ ! -f /tmp/pcscf_ip ]; then
    echo "Warning: P-CSCF not set after ${WAIT}s, starting asterisk anyway"
else
    PCSCF=$(cat /tmp/pcscf_ip)
    echo "P-CSCF ready after ${WAIT}s: $PCSCF, starting asterisk"
fi

# Watch IPsec and IMS registration for this COM line.
KEEP_IMS=/usr/local/bin/keep_ims.sh
if [ -f "$KEEP_IMS" ]; then
    echo "Starting IMS registration watchdog"
    sh "$KEEP_IMS" &
    LOOP_PID=$!
else
    echo "Warning: $KEEP_IMS missing - IPsec/IMS will not be watched"
fi

python3 /usr/local/bin/ami_usim.py /usr/local/etc/ami_usim.ini &
AMI_PID=$!

asterisk -f &
ASTERISK_PID=$!

wait "$ASTERISK_PID" || true
cleanup
