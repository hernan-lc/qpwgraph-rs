# Packaging and releases

Producing distributable artifacts. The release workflow builds the canonical
binary for the native tarball, Flatpak, and AppImage. Local packaging
instructions are in [`packaging/README.md`](../packaging/README.md).

## AppImage

Build the release binary and run:

```bash
bash packaging/appimage/build-appimage.sh 0.1.0 ./linuxdeploy-x86_64.AppImage
```

## Flatpak

```bash
flatpak-builder --force-clean --repo=repo builddir \
  packaging/io.github.nglmercer.qpwgraph-rs.yml
flatpak build-bundle repo qpwgraph-rs-0.1.0-x86_64.flatpak \
  io.github.nglmercer.qpwgraph-rs stable \
  --runtime-repo=https://releases.freedesktop-sdk.io/freedesktop-sdk.flatpakrepo
```

## Windows

Tagged releases also publish a portable Windows artifact named
`qpwgraph-rs-X.Y.Z-x86_64-pc-windows-msvc.zip` containing the executable,
README, and license.

## Related

- [Building](building.md) — the release builds these artifacts wrap.
