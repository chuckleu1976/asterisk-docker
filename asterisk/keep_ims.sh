#!/bin/sh
# Keep ePDG IPsec and IMS registration up for this asterisk instance.
# Started from entrypoint.sh. Safe to run as a detached helper.

trap 'exit 0' TERM INT
backoff=4
fails=0
unreg_fails=0
while true; do
    sleep 10
    if ! status=$(timeout -k 1 5 ipsec statusall 2>/dev/null); then
        continue
    fi
    if echo "$status" | grep -q ESTABLISHED; then
        fails=0
        if [ -S /var/run/asterisk/asterisk.ctl ]; then
            if timeout -k 1 5 asterisk -rx "pjsip show registrations" 2>/dev/null \
                | grep volte_ims | grep -q Registered; then
                unreg_fails=0
            else
                unreg_fails=$((unreg_fails + 1))
                echo "IMS not Registered (miss ${unreg_fails}) - sending REGISTER"
                timeout -k 1 5 asterisk -rx "pjsip send register volte_ims" >/dev/null 2>&1 || true
                if [ "$unreg_fails" -ge 3 ]; then
                    echo "IMS still down after ${unreg_fails} misses - reloading PJSIP"
                    PCSCF_IP=$(grep "received P-CSCF server IP" /var/log/strongswan-pcscf.log 2>/dev/null \
                        | tail -1 \
                        | sed -E 's/.*received P-CSCF server IP ([0-9a-f:]+).*/\1/')
                    if [ -n "$PCSCF_IP" ]; then
                        sed -i -E "s|match=[0-9a-f:]+|match=${PCSCF_IP}|g; s|ip=[0-9a-f:]+|ip=${PCSCF_IP}|g" /etc/asterisk/pjsip.conf || true
                    fi
                    timeout -k 1 8 asterisk -rx "module reload res_pjsip.so" >/dev/null 2>&1 || true
                    sleep 3
                    timeout -k 1 5 asterisk -rx "pjsip send register volte_ims" >/dev/null 2>&1 || true
                    unreg_fails=0
                fi
            fi
        fi
        continue
    fi
    if echo "$status" | grep -q CONNECTING; then
        continue
    fi
    fails=$((fails + 1))
    if [ "$fails" -lt 3 ]; then
        continue
    fi
    echo "Restarting ims after ${fails} failed checks, backoff=$backoff"
    if timeout -k 2 90 swanctl --initiate --child ims; then
        backoff=4
        fails=0
    else
        sleep $backoff
        backoff=$((backoff * 2 + (RANDOM & 1)))
    fi
done
