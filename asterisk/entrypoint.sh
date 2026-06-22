#!/bin/sh

set -e

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

ipsec start
sleep 2
swanctl --load-creds && swanctl --load-conns
# Initiate in background — ims.updown writes /tmp/pcscf_ip when ready.
swanctl --initiate --child ims &
(backoff=4; while true; do
     sleep 1;
     if ! swanctl --list-sas|grep '^ims:' > /dev/null; then
	 echo "Restarting ims, backoff=$backoff"
	 swanctl --initiate --child ims && backoff=4 || { sleep $backoff; backoff=$((backoff*2+(RANDOM&1))); }
     fi;
 done) &

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

# Power-cycle every SIM via pcscd so ami_usim.py gets a clean card state.
# Without this, a card left in the wrong AID/EF (e.g. by a host-side reader
# script, or by a previous incarnation of this container) causes IMS-AKA to
# fail and the carrier to reject REGISTER until the SIM is physically
# re-inserted.
echo "Resetting SIM card state before ami_usim.py..."
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

python3 /usr/local/bin/ami_usim.py /usr/local/etc/ami_usim.ini &
asterisk -f

