#!/bin/sh
# Cross-build a release binary for an aarch64 Alpine/postmarketOS device from
# an x86_64 host, against an Alpine sysroot (headers and .so files of the
# runtime dependencies), with clang + lld. The result links the sysroot's musl
# and libraries dynamically, like the packaged build.
#   SYSROOT=~/.cache/obscura-sysroot CLANG_BIN=~/.local/share/swiftly/bin \
#     [CARGO_SUBCOMMAND=clippy] build-aux/cross-build.sh [cargo args...]
# Output: target/aarch64-unknown-linux-musl/release/obscura
set -eu
cd "$(dirname "$0")/.."
sysroot=${SYSROOT:-$HOME/.cache/obscura-sysroot}
bin=${CLANG_BIN:-$HOME/.local/share/swiftly/bin}
triple=aarch64-unknown-linux-musl
wrappers=$PWD/target/cross-bin
mkdir -p "$wrappers"

# cc-rs and rustc pass their own --target; the Alpine triple must win so clang
# finds the sysroot's GCC and libstdc++ layout.
for tool in clang clang++; do
	cat > "$wrappers/$tool" <<-EOF
	#!/bin/sh
	for a do shift; case "\$a" in --target=*|-fuse-ld=*) ;; *) set -- "\$@" "\$a";; esac; done
	exec "$bin/$tool" --target=aarch64-alpine-linux-musl --sysroot="$sysroot" -fuse-ld=lld "\$@"
	EOF
	chmod +x "$wrappers/$tool"
done

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$wrappers/clang"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=-crt-static -C link-self-contained=no"
# `cargo test` runs the aarch64 test binary under qemu-user.
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUNNER="qemu-aarch64-static -L $sysroot"
export CC_aarch64_unknown_linux_musl="$wrappers/clang"
export CXX_aarch64_unknown_linux_musl="$wrappers/clang++"
export AR_aarch64_unknown_linux_musl="$bin/llvm-ar"
export PKG_CONFIG_SYSROOT_DIR="$sysroot"
export PKG_CONFIG_LIBDIR="$sysroot/usr/lib/pkgconfig:$sysroot/usr/share/pkgconfig"
export PKG_CONFIG_ALLOW_CROSS=1
# libcamera-sys runs bindgen: it needs a libclang and the target headers.
export LIBCLANG_PATH=${LIBCLANG_PATH:-$(ls -d "$HOME"/.local/share/swiftly/toolchains/*/usr/lib | tail -n1)}
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_linux_musl="--target=aarch64-alpine-linux-musl --sysroot=$sysroot"
export LOCALEDIR=${LOCALEDIR:-/usr/share/locale}

exec cargo "${CARGO_SUBCOMMAND:-build}" --release --target "$triple" "$@"
