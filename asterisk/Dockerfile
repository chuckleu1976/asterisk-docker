# Stage 0: custom pcsc-lite 1.9.9-sysmocom (protocol 4:4, matching pcscd server)
FROM pcscd-sysmocom AS pcsc

# Stage 1: use the existing image as source for pjproject and asterisk
# (gitea.sysmocom.de is protected by Anubis bot-protection and blocks git clone)
# --platform=linux/amd64 allows this stage to run on ARM64 (Pi 5) — only used
# for COPY of source files, no x86_64 binaries end up in the final image.
FROM --platform=linux/amd64 phcodercat/asterisk-vowifi AS source

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
COPY rpack_fix.py /tmp/rpack_fix.py
RUN cd asterisk && \
    make distclean || true && \
    sh bootstrap.sh && \
    ln -sf ../../../pjproject third-party/pjproject/source && \
    ./configure --enable-binary-modules && \
    make menuselect/menuselect menuselect-tree menuselect.makeopts && \
    ./menuselect/menuselect --enable codec_opus --disable BUILD_NATIVE && \
    python3 /tmp/rpack_fix.py && \
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
COPY sms_save.py /usr/local/bin/sms_save.py
RUN chmod +x /usr/local/bin/whisper_transcribe.py /usr/local/bin/sms_save.py
COPY p-cscf.conf /usr/local/etc/strongswan.d/charon/p-cscf.conf
COPY osmo-epdg.conf /usr/local/etc/strongswan.d/charon/osmo-epdg.conf
COPY ims.updown /usr/local/etc/ims.updown
COPY entrypoint.sh /entrypoint.sh
# Override system libpcsclite with our 1.9.9-sysmocom build (protocol 4:4)
# Fedora 44 pcsc-lite 2.4.1 ships libpcsclite.so.1 AND libpcsclite_real.so.1,
# both with SONAME=libpcsclite.so.1 — ldconfig picks the newer real file and
# overrides any symlink we create. Fix: replace both files directly, skip ldconfig.
COPY --from=pcsc /usr/lib64/libpcsclite.so.1.0.0 /tmp/libpcsclite_1.9.9.so
RUN ls -la /tmp/libpcsclite_1.9.9.so && \
    cp /tmp/libpcsclite_1.9.9.so /usr/lib64/libpcsclite.so.1 && \
    cp /tmp/libpcsclite_1.9.9.so /usr/lib64/libpcsclite_real.so.1 && \
    chmod 755 /usr/lib64/libpcsclite.so.1 /usr/lib64/libpcsclite_real.so.1 && \
    rm /tmp/libpcsclite_1.9.9.so && \
    ls -la /usr/lib64/libpcsclite*
ENTRYPOINT /entrypoint.sh
