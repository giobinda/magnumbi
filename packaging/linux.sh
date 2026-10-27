#!/bin/sh
# The desktop app's Linux packages (.deb where dpkg-deb is, .rpm where rpmbuild is), from a
# desktop build, into target/packages:
#   cargo build --release --features desktop && packaging/linux.sh 0.1.6
# The window needs WebKitGTK 4.1 from the system; OpenSSL is built in.
set -eu
version=$1
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
out=$root/target/packages
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

stage() {
    install -Dm755 -s "$root/target/release/magnum-bi" "$1/usr/bin/magnum-bi"
    install -Dm644 "$here/magnum-bi.desktop" "$1/usr/share/applications/magnum-bi.desktop"
    install -Dm644 "$here/icon.png" "$1/usr/share/icons/hicolor/512x512/apps/magnum-bi.png"
    install -Dm644 "$here/icon.svg" "$1/usr/share/icons/hicolor/scalable/apps/magnum-bi.svg"
    install -Dm644 "$root/LICENSE" "$1/usr/share/licenses/magnum-bi/LICENSE"
}

if command -v dpkg-deb >/dev/null; then
    arch=$(dpkg --print-architecture)
    stage "$work/deb"
    mkdir -p "$work/deb/DEBIAN"
    cat > "$work/deb/DEBIAN/control" <<EOF
Package: magnum-bi
Version: $version
Architecture: $arch
Maintainer: Magnum BI <https://github.com/giobinda/magnumbi>
Depends: libwebkit2gtk-4.1-0
Section: misc
Priority: optional
Homepage: https://giobinda.github.io/magnumbi/
Description: semantic models and dashboards, on this computer
 Magnum BI lite in a window of its own, with its own PostgreSQL.
EOF
    dpkg-deb --root-owner-group --build "$work/deb" "$out/magnum-bi_${version}_$arch.deb"
fi

if command -v rpmbuild >/dev/null; then
    stage "$work/rpm"
    # what it requires comes from the program itself (libwebkit2gtk-4.1.so.0, …), so it fits any
    # distribution's package names
    cat > "$work/magnum-bi.spec" <<EOF
Name: magnum-bi
Version: $version
Release: 1
Summary: Semantic models and dashboards, on this computer
License: AGPL-3.0-only
URL: https://giobinda.github.io/magnumbi/
%global debug_package %{nil}
%global source_date_epoch_from_changelog 0
%global __brp_strip %{nil}
%description
Magnum BI lite in a window of its own, with its own PostgreSQL.
%install
cp -a $work/rpm/. %{buildroot}/
%files
/usr/bin/magnum-bi
/usr/share/applications/magnum-bi.desktop
/usr/share/icons/hicolor/512x512/apps/magnum-bi.png
/usr/share/icons/hicolor/scalable/apps/magnum-bi.svg
%license /usr/share/licenses/magnum-bi/LICENSE
EOF
    rpmbuild -bb --quiet --define "_topdir $work/rpmbuild" --define "_rpmdir $out" \
        --define '_build_name_fmt %%{NAME}-%%{VERSION}-%%{RELEASE}.%%{ARCH}.rpm' "$work/magnum-bi.spec"
fi
ls "$out"
