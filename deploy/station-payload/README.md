# The station's build for the image

The release workflow (`.github/workflows/release.yml`) puts the station's signed build here —
`duowave-station-aarch64.tar.gz` and its `.sig` — before it builds the gateway's image: `Dockerfile.prod` then builds it
into the program (`DUOWAVE_STATION_PAYLOAD`, `gateway/build.rs`), so the panel can move the gateway onto a station
without a file to find. Without it the image's gateway downloads the station's build of its own version from the release
and checks its signature (`gateway/src/placement/payload.rs`). Nothing here is tracked but this file.
