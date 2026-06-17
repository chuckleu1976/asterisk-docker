#!/bin/sh

set -e

sleep 3

ipsec start
sleep 2
swanctl --load-creds && swanctl --load-conns && swanctl --initiate --child ims
swanctl --log 2>&1 | tee -a /var/log/strongswan.log &
(backoff=4; while true; do
     sleep 1;
     if ! swanctl --list-sas|grep '^ims:' > /dev/null; then
	 echo "Restarting ims, backoff=$backoff"
	 swanctl --initiate --child ims && backoff=4 || { sleep $backoff; backoff=$((backoff*2+(RANDOM&1))); }
     fi;
 done) &

# Wait for ims.updown to write a real P-CSCF IP into pjsip.conf before
# starting asterisk, so the first REGISTER attempt uses the correct address.
# ims.updown sets P-CSCF in /etc/asterisk/pjsip.conf after tunnel is up.
echo "Waiting for P-CSCF IP to be set in pjsip.conf..."
WAIT=0
while grep -q 'ip=::1' /etc/asterisk/pjsip.conf 2>/dev/null && [ $WAIT -lt 120 ]; do
    sleep 3
    WAIT=$((WAIT + 3))
done
if grep -q 'ip=::1' /etc/asterisk/pjsip.conf 2>/dev/null; then
    echo "Warning: P-CSCF not set after ${WAIT}s, starting asterisk anyway"
else
    echo "P-CSCF ready after ${WAIT}s, starting asterisk"
fi

python3 /usr/local/bin/ami_usim.py /usr/local/etc/ami_usim.ini &
asterisk -f

