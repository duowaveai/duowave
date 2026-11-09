# The release's own programs for the image

The release workflow (`.github/workflows/release.yml`, job `gateway-image`) puts here the gateway it has already built
for its Linux archives — `amd64/duowave` and `arm64/duowave`, static musl builds with the update feed and the station's
signed build inside — and builds `Dockerfile.prod` with `GATEWAY=prebuilt`: the image then carries the very program of
the archive instead of compiling it twice more. Without that argument (compose, the Home Assistant add-on) the image
builds the program itself and this folder is not read. Nothing here is tracked but this file.
