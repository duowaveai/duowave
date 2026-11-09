ARG BUILD_FROM
FROM ${BUILD_FROM}
# Home Assistant's /data belongs to root: the add-on runs the gateway as root, inside its container only.
USER root
COPY run.sh /run.sh
RUN chmod 755 /run.sh
# The port is the add-on's option, not the image's PORT: the health check takes it from run.sh, as the gateway does.
HEALTHCHECK --interval=30s --timeout=5s --start-period=40s --retries=3 CMD ["/run.sh", "health"]
CMD ["/run.sh"]
