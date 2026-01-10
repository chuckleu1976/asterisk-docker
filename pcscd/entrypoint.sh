#!/bin/sh

rm -f /run/pcscd/pcscd.*
mkdir /run/dbus
dbus-daemon --system
/usr/lib/polkit-1/polkitd &
pcscd -f
