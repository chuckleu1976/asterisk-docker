#!/bin/sh

rm -f /run/pcscd/pcscd.*
mkdir /run/dbus
dbus-daemon --system
/usr/lib/polkit-1/polkitd &

if [ "${SIM_MODE:-remote}" = "local" ]; then
    # Local USB reader: skip remsim config, let pcscd auto-detect USB readers
    mkdir -p /tmp/empty-reader-conf
    exec /usr/sbin/pcscd -f -c /tmp/empty-reader-conf
else
    # Remote SIM via osmo-remsim IFD handler (default)
    exec /usr/sbin/pcscd -f
fi
