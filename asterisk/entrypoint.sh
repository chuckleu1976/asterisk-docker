#!/bin/sh

set -e

sleep 3

ipsec start
sleep 2
swanctl --load-creds && swanctl --load-conns && swanctl --initiate --child ims
swanctl --log &
(backoff=4; while true; do
     sleep 1;
     if ! swanctl --list-sas|grep '^ims:' > /dev/null; then
	 echo "Restarting ims, backoff=$backoff"
	 swanctl --initiate --child ims && backoff=4 || { sleep $backoff; backoff=$((backoff*2+(RANDOM&1))); }
     fi;
 done) &
python3 /usr/local/bin/ami_usim.py /usr/local/etc/ami_usim.ini &
asterisk -f

