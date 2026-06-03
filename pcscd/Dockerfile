# ---- Stage 1: Builder ----
FROM fedora:latest AS builder

RUN dnf -y install gcc make autoconf automake libtool pkg-config \
    libusb1-devel systemd-devel flex gettext

# Build pcsc-lite 1.9.9-sysmocom (256 readers)
COPY pcsc-lite/ /build/pcsc-lite/
WORKDIR /build/pcsc-lite
RUN chmod +x configure && \
    make distclean || true && \
    ./configure --prefix=/usr --sysconfdir=/etc --localstatedir=/var \
      --disable-documentation --disable-polkit && \
    make -j$(nproc) && \
    make install DESTDIR=/install && \
    make install

# Build ccid 1.5.0-sysmocom (USB CCID driver)
COPY ccid/ /build/ccid/
WORKDIR /build/ccid
RUN chmod +x configure && \
    find . -name "*.pl" -exec chmod +x {} + && \
    make distclean || true && \
    ./configure --prefix=/usr --sysconfdir=/etc && \
    make -j$(nproc) && \
    make install DESTDIR=/install

# Build pcsc-tools 1.7.3 (linked against our libpcsclite)
COPY pcsc-tools-1.7.3/ /build/pcsc-tools/
WORKDIR /build/pcsc-tools
RUN chmod +x configure && \
    make distclean || true && \
    ./configure --prefix=/usr && \
    make -j$(nproc) && \
    make install DESTDIR=/install

# ---- Stage 2: Runtime ----
FROM fedora:latest

RUN dnf -y install dbus-daemon libusb1 systemd-libs polkit && \
    rm -f /usr/sbin && mkdir -p /usr/sbin

COPY --from=builder /install/ /
RUN printf '#!/bin/sh\nrm -f /run/pcscd/pcscd.*\nmkdir /run/dbus\ndbus-daemon --system\n/usr/lib/polkit-1/polkitd &\n\nif [ "${SIM_MODE:-remote}" = "local" ]; then\n    mkdir -p /tmp/empty-reader-conf\n    exec pcscd -f --reader-conf-dir /tmp/empty-reader-conf\nelse\n    exec pcscd -f\nfi\n' > /entrypoint.sh && \
    chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
