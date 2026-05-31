# Stage 1: use the existing image as source for pjproject and asterisk
# (gitea.sysmocom.de is protected by Anubis bot-protection and blocks git clone)
FROM phcodercat/asterisk-vowifi AS source

# Stage 2: fresh build from source
FROM fedora:latest

WORKDIR /home/asterisk-build
COPY patches/strongswan/01_configure_gettext.patch /home/asterisk-build/01_configure_gettext.patch
COPY patches/strongswan/02_warnings.patch /home/asterisk-build/02_warnings.patch
RUN dnf -y update && \
    dnf -y install git gcc g++ openssl-devel patch libedit-devel opus-devel libuuid-devel autoconf automake libtool gettext perl python3 flex bison pkg-config m4 gettext-devel gperf gmp-devel pcsc-lite-devel jansson-devel libxml2-devel sqlite-devel opencore-amr-devel vo-amrwbenc-devel libmnl-devel speex-devel speexdsp-devel libvorbis-devel alsa-lib-devel portaudio-devel libcurl-devel xmlstarlet postgresql-devel unixODBC-devel neon-devel gmime-devel lua-devel uriparser-devel libxslt-devel mysql-devel bluez-libs-devel radcli-devel freetds-devel jack-audio-connection-kit-devel libcap-devel net-snmp-devel corosynclib-devel newt-devel popt-devel libical-devel spandsp-devel openldap-devel codec2-devel fftw-devel libsndfile-devel unbound-devel wget subversion libresample-devel binutils-devel libsrtp-devel gsm-devel iproute pcsc-tools iproute procps-ng net-tools python3-pysocks python3-requests python3-pyscard
# Copy pre-checked-out sources from the existing image (cherry-pick already applied)
COPY --from=source /home/asterisk-build/pjproject /home/asterisk-build/pjproject
COPY --from=source /home/asterisk-build/asterisk /home/asterisk-build/asterisk
RUN cd asterisk && \
    make distclean || true && \
    sh bootstrap.sh && \
    ln -sf ../../../pjproject third-party/pjproject/source && \
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
RUN pip install panoramisk faster-whisper
# Pre-download the model using huggingface_hub (avoids network access at runtime)
RUN python3 -c "from huggingface_hub import snapshot_download; snapshot_download('Systran/faster-whisper-tiny')"
COPY ami_usim.py /usr/local/bin/ami_usim.py
COPY send_notification.py /usr/local/bin/send_notification.py
COPY whisper_transcribe.py /usr/local/bin/whisper_transcribe.py
RUN chmod +x /usr/local/bin/whisper_transcribe.py
COPY p-cscf.conf /usr/local/etc/strongswan.d/charon/p-cscf.conf
COPY osmo-epdg.conf /usr/local/etc/strongswan.d/charon/osmo-epdg.conf
COPY ims.updown /usr/local/etc/ims.updown
COPY entrypoint.sh /entrypoint.sh
ENTRYPOINT /entrypoint.sh
