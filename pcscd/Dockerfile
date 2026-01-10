FROM fedora:latest

COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT /entrypoint.sh
RUN dnf -y update && \
    dnf -y install pcsc-lite pcsc-tools dbus-daemon


