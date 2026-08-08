#!/usr/bin/env bash
set -euo pipefail

version="${1:?usage: build-appimage.sh VERSION LINUXDEPLOY [OUTPUT_DIR]}"
linuxdeploy="${2:?usage: build-appimage.sh VERSION LINUXDEPLOY [OUTPUT_DIR]}"
output_dir="${3:-dist}"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
app_id="io.github.nglmercer.qpwgraph-rs"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/qpwgraph-rs-appimage.XXXXXX")"
app_dir="$work_dir/AppDir"
output_dir="$repo_root/$output_dir"

cleanup() {
    rm -rf -- "$work_dir"
}
trap cleanup EXIT

install -Dm755 "$repo_root/target/release/qpwgraph-rs" \
    "$app_dir/usr/bin/qpwgraph-rs"
install -Dm644 "$repo_root/packaging/$app_id.desktop" \
    "$app_dir/usr/share/applications/$app_id.desktop"
install -Dm644 "$repo_root/packaging/$app_id.svg" \
    "$app_dir/usr/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 "$repo_root/packaging/$app_id.metainfo.xml" \
    "$app_dir/usr/share/metainfo/$app_id.metainfo.xml"
install -Dm644 "$repo_root/README.md" \
    "$app_dir/usr/share/doc/qpwgraph-rs/README.md"
install -Dm644 "$repo_root/LICENSE" \
    "$app_dir/usr/share/licenses/qpwgraph-rs/LICENSE"
install -Dm644 "$repo_root/vendor/audiopus_sys/LICENSE.md" \
    "$app_dir/usr/share/licenses/qpwgraph-rs/audiopus_sys-LICENSE.md"
install -Dm644 "$repo_root/vendor/audiopus_sys/opus/COPYING" \
    "$app_dir/usr/share/licenses/qpwgraph-rs/opus-COPYING"
install -Dm644 "$repo_root/vendor/libspa/LICENSE" \
    "$app_dir/usr/share/licenses/qpwgraph-rs/libspa-LICENSE"
install -Dm644 "$repo_root/vendor/nnnoiseless/COPYING" \
    "$app_dir/usr/share/licenses/qpwgraph-rs/nnnoiseless-COPYING"

mkdir -p "$output_dir"
(
    cd "$work_dir"
    APPIMAGE_EXTRACT_AND_RUN=1 "$linuxdeploy" \
        --appdir "$app_dir" \
        --executable "$app_dir/usr/bin/qpwgraph-rs" \
        --desktop-file "$app_dir/usr/share/applications/$app_id.desktop" \
        --icon-file "$app_dir/usr/share/icons/hicolor/scalable/apps/$app_id.svg" \
        --output appimage
)

appimage="$(find "$work_dir" -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
if [[ -z "$appimage" ]]; then
    echo "linuxdeploy did not produce an AppImage" >&2
    exit 1
fi

install -Dm755 "$appimage" \
    "$output_dir/qpwgraph-rs-${version}-x86_64.AppImage"
