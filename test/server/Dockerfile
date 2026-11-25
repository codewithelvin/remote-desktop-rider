FROM ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive container=docker
RUN apt-get update \
 && apt-get install -y --no-install-recommends systemd systemd-sysv ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && systemctl mask systemd-logind getty.target console-getty.service systemd-udevd systemd-resolved
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
