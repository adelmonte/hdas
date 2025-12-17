# Maintainer: Your Name <your@email.com>
pkgname=hdas
pkgver=0.1.0
pkgrel=1
pkgdesc="Track which packages create files in your home directory using eBPF"
arch=('x86_64')
url="https://github.com/adelmonte/hdas"
license=('GPL-3.0-only')
depends=('libbpf')

package() {
    install -Dm755 "$startdir/target/release/hdas" "$pkgdir/usr/bin/hdas"
    install -Dm644 "$startdir/hdas@.service" "$pkgdir/usr/lib/systemd/system/hdas@.service"
}
