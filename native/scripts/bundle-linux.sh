#!/usr/bin/env bash
# Package the Linux build of Aran Terminal: .tar.gz, .deb, .rpm and .AppImage.
#
#   scripts/bundle-linux.sh            # builds target/release/aran first
#   SKIP_BUILD=1 scripts/bundle-linux.sh
#
# Needs: dpkg-deb (dpkg), rpmbuild (rpm), curl + file (for appimagetool). Any
# packager that is missing is skipped with a warning, so a partial toolchain still
# yields the formats it can make. Output lands in dist/.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)                # x86_64 / aarch64
case "$ARCH" in
  x86_64)  DEB_ARCH=amd64 ;;
  aarch64) DEB_ARCH=arm64 ;;
  *)       DEB_ARCH="$ARCH" ;;
esac
PKG=aran-terminal
ICON_SRC=../conductor/src-tauri/icons/icon.png      # 512×512
ICON_128=../conductor/src-tauri/icons/128x128.png
DIST=dist
BASE="Aran-Terminal_${VERSION}_linux_${ARCH}"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release
fi
BIN=target/release/aran
[[ -x "$BIN" ]] || { echo "missing $BIN" >&2; exit 1; }

rm -rf "$DIST"
mkdir -p "$DIST"

# ---- shared file tree (FHS layout used by every format) -------------------------
TREE=$(mktemp -d)
install -Dm755 "$BIN" "$TREE/usr/bin/aran"
install -Dm644 "$ICON_SRC" "$TREE/usr/share/icons/hicolor/512x512/apps/$PKG.png"
install -Dm644 "$ICON_128" "$TREE/usr/share/icons/hicolor/128x128/apps/$PKG.png"
install -Dm644 /dev/stdin "$TREE/usr/share/applications/$PKG.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Aran Terminal
Comment=Terminal manager with a focus dashboard for AI coding agents
Exec=aran
Icon=$PKG
Terminal=false
Categories=Development;TerminalEmulator;
Keywords=terminal;agent;claude;focus;
StartupWMClass=aran
EOF
install -Dm644 README.md "$TREE/usr/share/doc/$PKG/README.md"
install -Dm644 ../LICENSE "$TREE/usr/share/doc/$PKG/LICENSE"

# ---- .tar.gz (portable) -----------------------------------------------------------
STAGE=$(mktemp -d)
mkdir -p "$STAGE/$BASE"
cp "$BIN" README.md "$STAGE/$BASE/"
cp "$ICON_128" "$STAGE/$BASE/aran.png"
tar -C "$STAGE" -czf "$DIST/$BASE.tar.gz" "$BASE"
rm -rf "$STAGE"

# ---- .deb --------------------------------------------------------------------------
if command -v dpkg-deb >/dev/null; then
  DEB=$(mktemp -d)
  cp -a "$TREE/." "$DEB/"
  mkdir -p "$DEB/DEBIAN"
  SIZE_KB=$(du -sk "$TREE" | cut -f1)
  cat > "$DEB/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION
Section: devel
Priority: optional
Architecture: $DEB_ARCH
Installed-Size: $SIZE_KB
Maintainer: Arun Karnanithi <https://github.com/arunkarnann>
Homepage: https://github.com/arunkarnann/aran-terminal
Depends: libc6, libgl1, libxkbcommon0, libx11-6, libxcursor1, libxi6, libxrandr2, libwayland-client0
Description: Terminal manager with a focus dashboard for AI coding agents
 Aran Terminal wraps real PTY-backed terminals and watches each session to tell
 you which one actually needs you (RUNNING / IDLE / WAITING). Pure Rust, no webview.
EOF
  chmod 0755 "$DEB/DEBIAN"
  find "$DEB" -type d -exec chmod 0755 {} +
  dpkg-deb --build --root-owner-group "$DEB" "$DIST/${PKG}_${VERSION}_${DEB_ARCH}.deb"
  rm -rf "$DEB"
else
  echo "warning: dpkg-deb not found; skipping .deb" >&2
fi

# ---- .rpm --------------------------------------------------------------------------
if command -v rpmbuild >/dev/null; then
  RPM=$(mktemp -d)
  mkdir -p "$RPM"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT}
  cat > "$RPM/SPECS/$PKG.spec" <<EOF
Name:           $PKG
Version:        $VERSION
Release:        1
Summary:        Terminal manager with a focus dashboard for AI coding agents
License:        MIT
URL:            https://github.com/arunkarnann/aran-terminal
BuildArch:      $ARCH
AutoReqProv:    no
Requires:       mesa-libGL, libxkbcommon, libX11, libXcursor, libXi, libXrandr, libwayland-client

%description
Aran Terminal wraps real PTY-backed terminals and watches each session to tell
you which one actually needs you (RUNNING / IDLE / WAITING). Pure Rust, no webview.

%install
cp -a "$TREE/." %{buildroot}/

%files
/usr/bin/aran
/usr/share/applications/$PKG.desktop
/usr/share/icons/hicolor/512x512/apps/$PKG.png
/usr/share/icons/hicolor/128x128/apps/$PKG.png
%doc /usr/share/doc/$PKG/README.md
%license /usr/share/doc/$PKG/LICENSE
EOF
  rpmbuild -bb --define "_topdir $RPM" --define "_build_id_links none" \
    --define "__strip /bin/true" "$RPM/SPECS/$PKG.spec" >/dev/null
  cp "$RPM"/RPMS/*/*.rpm "$DIST/${PKG}-${VERSION}-1.${ARCH}.rpm"
  rm -rf "$RPM"
else
  echo "warning: rpmbuild not found; skipping .rpm" >&2
fi

# ---- .AppImage ---------------------------------------------------------------------
APPIMAGETOOL="${APPIMAGETOOL:-}"
if [[ -z "$APPIMAGETOOL" ]] && command -v curl >/dev/null; then
  APPIMAGETOOL="$(mktemp -d)/appimagetool"
  if ! curl -fsSL --retry 3 -o "$APPIMAGETOOL" \
      "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${ARCH}.AppImage"; then
    APPIMAGETOOL=""
  else
    chmod +x "$APPIMAGETOOL"
  fi
fi
if [[ -n "$APPIMAGETOOL" ]]; then
  APPDIR=$(mktemp -d)/AppDir
  mkdir -p "$APPDIR"
  cp -a "$TREE/." "$APPDIR/"
  cp "$TREE/usr/share/applications/$PKG.desktop" "$APPDIR/"
  cp "$ICON_SRC" "$APPDIR/$PKG.png"
  cp "$ICON_SRC" "$APPDIR/.DirIcon"
  cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/aran" "$@"
EOF
  chmod +x "$APPDIR/AppRun"
  # --appimage-extract-and-run: no FUSE needed on CI runners.
  ARCH="$ARCH" "$APPIMAGETOOL" --appimage-extract-and-run -n "$APPDIR" \
    "$DIST/Aran-Terminal_${VERSION}_${ARCH}.AppImage" >/dev/null 2>&1 \
    || ARCH="$ARCH" "$APPIMAGETOOL" -n "$APPDIR" "$DIST/Aran-Terminal_${VERSION}_${ARCH}.AppImage"
  rm -rf "$(dirname "$APPDIR")"
else
  echo "warning: appimagetool unavailable; skipping .AppImage" >&2
fi

rm -rf "$TREE"
echo "Built:"
ls -lh "$DIST"
