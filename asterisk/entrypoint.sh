#!/bin/sh

set -e

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

# Power-cycle every SIM via pcscd before IKE initiation.
# strongSwan eap-sim-pcsc talks to the SIM directly via pcsc-lite during
# IKE_AUTH (EAP-AKA exchange with the ePDG). A card left in the wrong
# AID/EF state causes the AKA challenge to fail, delaying tunnel setup by
# multiple retry cycles (exponential backoff: 4, 8, 16... s).
echo "Resetting SIM card state before IKE initiation..."
python3 - <<'PYRESET' || true
from smartcard.System import readers
from smartcard.CardConnection import CardConnection
for i, r in enumerate(readers()):
    try:
        c = r.createConnection()
        c.connect()
        c.reconnect(disposition=CardConnection.RESET)
        c.disconnect()
        print(f"  P{i}: reset OK ({r.name})")
    except Exception as e:
        print(f"  P{i}: reset skipped ({e})")
PYRESET

# Initiate IKE in background — ims.updown writes /tmp/pcscf_ip when ready.
swanctl --initiate --child ims &
(
    trap 'exit 0' TERM INT
    backoff=4
    while true; do
        sleep 1
        if ! swanctl --list-sas | grep '^ims:' > /dev/null; then
            echo "Restarting ims, backoff=$backoff"
            swanctl --initiate --child ims && backoff=4 || { sleep $backoff; backoff=$((backoff * 2 + (RANDOM & 1))); }
        fi
    done
) &
LOOP_PID=$!

# Wait for ims.updown to signal P-CSCF is ready via /tmp/pcscf_ip file.
# This is written by ims.updown after it receives the P-CSCF from strongSwan.
echo "Waiting for P-CSCF IP to be set in pjsip.conf..."
rm -f /tmp/pcscf_ip
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

python3 /usr/local/bin/ami_usim.py /usr/local/etc/ami_usim.ini &
AMI_PID=$!

# Rebuild res_pjsip_messaging.so from the already-patched source. The image
# contains the rpack_fix.py patch in the source tree, but the pre-compiled
# module in /usr/lib/asterisk/modules is unpatched. Rebuilding here ensures
# every container start loads the patched module so RP-ACK/DELIVERY-REPORT
# responses reuse the established VoLTE TCP connection instead of failing
# with EADDRINUSE.
if [ -f /home/asterisk-build/asterisk/res/res_pjsip_messaging.c ]; then
    echo "Rebuilding patched res_pjsip_messaging.so..."
    cd /home/asterisk-build/asterisk
    touch res/res_pjsip_messaging.c
    make res -j$(nproc) 2>&1 | tail -5
    cp -f res/res_pjsip_messaging.so /usr/lib/asterisk/modules/
    echo "Patched res_pjsip_messaging.so installed."
fi

asterisk -f &
ASTERISK_PID=$!

wait "$ASTERISK_PID" || true
cleanup

