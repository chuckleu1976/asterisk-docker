FROM fedora:latest

WORKDIR /home/asterisk-build
COPY patches/strongswan/01_configure_gettext.patch /home/asterisk-build/01_configure_gettext.patch
COPY patches/strongswan/02_warnings.patch /home/asterisk-build/02_warnings.patch
RUN dnf -y update && \
    dnf -y install git gcc g++ openssl-devel patch libedit-devel opus-devel libuuid-devel autoconf automake libtool gettext perl python3 flex bison pkg-config m4 gettext-devel gperf gmp-devel pcsc-lite-devel jansson-devel libxml2-devel sqlite-devel opencore-amr-devel vo-amrwbenc-devel libmnl-devel speex-devel speexdsp-devel libvorbis-devel alsa-lib-devel portaudio-devel libcurl-devel xmlstarlet postgresql-devel unixODBC-devel neon-devel gmime-devel lua-devel uriparser-devel libxslt-devel mysql-devel bluez-libs-devel radcli-devel freetds-devel jack-audio-connection-kit-devel libcap-devel net-snmp-devel corosynclib-devel newt-devel popt-devel libical-devel spandsp-devel openldap-devel codec2-devel fftw-devel libsndfile-devel unbound-devel wget subversion libresample-devel binutils-devel libsrtp-devel gsm-devel iproute pcsc-tools iproute procps-ng net-tools python3-pysocks python3-requests python3-pyscard
RUN git clone --depth=1 --branch sysmocom/2.14 https://gitea.sysmocom.de/sysmocom/pjproject.git && git clone --depth=1 --branch sysmocom/20.7.0 https://gitea.sysmocom.de/sysmocom/asterisk.git  && \
    cd asterisk && \
    git config user.email "autobuilder@github.com" && \
    git config user.name "Auto builder" && \
    git fetch origin f1b60dcd9568c4045512fd0d8b619b9fb91a7f35 && \
    git cherry-pick f1b60dcd9568c4045512fd0d8b619b9fb91a7f35 && \
    ln -s ../../../pjproject third-party/pjproject/source && \
    sh bootstrap.sh && \
    ./configure --enable-binary-modules && \
    make menuselect/menuselect menuselect-tree menuselect.makeopts && \
    ./menuselect/menuselect --enable codec_opus --disable BUILD_NATIVE && \
    make -j$(nproc) && \
    make install
RUN git clone --depth=1 --branch jolly/work https://gitea.osmocom.org/ims-volte-vowifi/strongswan-epdg.git && \
    cd strongswan-epdg && \
    patch -i ../01_configure_gettext.patch -p1 && \
    patch -i ../02_warnings.patch -p1 && \
    autoreconf -if && \
    ./configure --enable-eap-sim-pcsc --enable-p-cscf --enable-save-keys --enable-eap-sim --enable-eap-aka --enable-kernel-libipsec && \
    make -j$(nproc) && \
    make install
RUN pip install panoramisk
COPY ami_usim.py /usr/local/bin/ami_usim.py
COPY send_notification.py /usr/local/bin/send_notification.py
COPY p-cscf.conf /usr/local/etc/strongswan.d/charon/p-cscf.conf
COPY osmo-epdg.conf /usr/local/etc/strongswan.d/charon/osmo-epdg.conf
COPY ims.updown /usr/local/etc/ims.updown
COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT /entrypoint.sh
