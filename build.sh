#!/usr/bin/env bash
# sipress build script — CLI (cargo-zigbuild) + GUI (Tauri)
# All outputs land in dist/
set -euo pipefail

PROFILE="release"
OUT="dist"
BIN="sipress"
GUI_DIR="gui"
GUI_BIN="sipress-gui"
GUI_TARGET_DIR="target/release"
GUI_BUNDLE_DIR="$GUI_TARGET_DIR/bundle"

# ── Colored output ────────────────────────────────────────────────
info() { echo -e "\033[36m[sipress]\033[0m $*"; }
ok()   { echo -e "\033[32m[  OK  ]\033[0m $*"; }
warn() { echo -e "\033[33m[ WARN ]\033[0m $*"; }
err()  { echo -e "\033[31m[ERROR ]\033[0m $*" >&2; exit 1; }

# ── Tool checks ───────────────────────────────────────────────────
ensure_zig() {
    if command -v zig &>/dev/null; then
        info "zig $(zig version) found in PATH"; return
    fi
    local zig_dir
    zig_dir=$(python -c "import ziglang, os; print(os.path.dirname(ziglang.__file__))" 2>/dev/null || true)
    if [[ -n "$zig_dir" && ( -f "$zig_dir/zig" || -f "$zig_dir/zig.exe" ) ]]; then
        export PATH="$zig_dir:$PATH"
        info "zig $(zig version) (ziglang package: $zig_dir)"
    else
        err "zig not found. Install: pip install ziglang  OR  download from ziglang.org"
    fi
}

ensure_zigbuild() {
    command -v cargo-zigbuild &>/dev/null \
        || err "cargo-zigbuild not found. Install: cargo install cargo-zigbuild"
}

ensure_node() {
    command -v node &>/dev/null || err "node not found. Install from https://nodejs.org"
    command -v npm  &>/dev/null || err "npm not found. Install from https://nodejs.org"
    info "node $(node --version)  npm $(npm --version)"
}

mkdir -p "$OUT"

# ── CLI: cross-compile via cargo-zigbuild ────────────────────────
build_cross() {
    local target="$1" out_name="$2"
    info "Cross-compiling $target ..."
    cargo zigbuild \
        --manifest-path cli/Cargo.toml \
        --target "$target" \
        --$PROFILE -q
    local src="target/$target/$PROFILE/$BIN"
    [[ "$out_name" == *.exe ]] && src="${src}.exe"
    cp "$src" "$OUT/$out_name"
    local size; size=$(du -sh "$OUT/$out_name" | cut -f1)
    ok "$OUT/$out_name  ($size)"
}

# ── CLI: native build ────────────────────────────────────────────
build_native() {
    info "Native CLI build ..."
    cargo build --manifest-path cli/Cargo.toml --$PROFILE -q
    local src="target/$PROFILE/$BIN"
    local out_name

    # Normalize OS/arch names: darwin→macos, aarch64→arm64
    local os_tag arch_tag
    case "$(uname -s)" in
        Darwin) os_tag="macos" ;;
        Linux)  os_tag="linux" ;;
        *)      os_tag="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
    esac
    case "$(uname -m)" in
        x86_64)        arch_tag="x86_64" ;;
        arm64|aarch64) arch_tag="arm64"  ;;
        *)             arch_tag="$(uname -m)" ;;
    esac

    if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" || "$OSTYPE" == "win32" ]]; then
        src="${src}.exe"; out_name="${BIN}-windows-x86_64-native.exe"
    else
        out_name="${BIN}-${os_tag}-${arch_tag}"
    fi

    cp "$src" "$OUT/$out_name"
    local size; size=$(du -sh "$OUT/$out_name" | cut -f1)
    ok "$OUT/$out_name  ($size)"
}

# ── GUI: copy one file to dist with a nice name ──────────────────
_copy_to_dist() {
    local src="$1" dst_name="$2"
    if [[ -f "$src" ]]; then
        cp "$src" "$OUT/$dst_name"
        local size; size=$(du -sh "$OUT/$dst_name" | cut -f1)
        ok "$OUT/$dst_name  ($size)"
    else
        warn "Not found (skipped): $src"
    fi
}

# ── GUI: build Tauri + collect all outputs into dist/ ────────────
build_gui() {
    ensure_node
    info "npm install ..."
    npm install --prefix "$GUI_DIR" --silent

    info "Tauri build (may take a few minutes) ..."
    npm run --prefix "$GUI_DIR" tauri build

    local os; os=$(uname -s | tr '[:upper:]' '[:lower:]')
    local arch; arch=$(uname -m)

    case "$os" in
        mingw*|msys*|cygwin*|windows*)
            # Windows: MSI installer + NSIS setup + portable exe
            local msi; msi=$(find "$GUI_BUNDLE_DIR/msi"   -name "*.msi" 2>/dev/null | head -1)
            local nsis; nsis=$(find "$GUI_BUNDLE_DIR/nsis" -name "*.exe" 2>/dev/null | head -1)
            _copy_to_dist "$msi"  "${GUI_BIN}-windows-x86_64-installer.msi"
            _copy_to_dist "$nsis" "${GUI_BIN}-windows-x86_64-setup.exe"
            _copy_to_dist "$GUI_TARGET_DIR/${GUI_BIN}.exe" "${GUI_BIN}-windows-x86_64-portable.exe"
            ;;
        linux*)
            # Linux: deb (installer) + AppImage (portable)
            local deb; deb=$(find "$GUI_BUNDLE_DIR/deb"      -name "*.deb"      2>/dev/null | head -1)
            local ai;  ai=$(find  "$GUI_BUNDLE_DIR/appimage" -name "*.AppImage" 2>/dev/null | head -1)
            _copy_to_dist "$deb" "${GUI_BIN}-linux-${arch}-installer.deb"
            _copy_to_dist "$ai"  "${GUI_BIN}-linux-${arch}-portable.AppImage"
            ;;
        darwin*)
            # macOS: dmg (installer) + .app binary (portable)
            local dmg; dmg=$(find "$GUI_BUNDLE_DIR/dmg"   -name "*.dmg" 2>/dev/null | head -1)
            # .app 名稱取自 tauri.conf.json 的 productName（sipress），內含的執行檔為 cargo bin 名稱
            local app_bin="$GUI_BUNDLE_DIR/macos/sipress.app/Contents/MacOS/${GUI_BIN}"
            _copy_to_dist "$dmg"     "${GUI_BIN}-macos-${arch}-installer.dmg"
            _copy_to_dist "$app_bin" "${GUI_BIN}-macos-${arch}-portable"
            ;;
    esac
}

TARGET="${1:-all}"

case "$TARGET" in
    linux-x86)
        ensure_zig; ensure_zigbuild
        rustup target add x86_64-unknown-linux-musl 2>/dev/null || true
        build_cross "x86_64-unknown-linux-musl" "sipress-linux-x86_64"
        ;;
    linux-arm64)
        ensure_zig; ensure_zigbuild
        rustup target add aarch64-unknown-linux-musl 2>/dev/null || true
        build_cross "aarch64-unknown-linux-musl" "sipress-linux-arm64"
        ;;
    windows)
        ensure_zig; ensure_zigbuild
        rustup target add x86_64-pc-windows-gnu 2>/dev/null || true
        build_cross "x86_64-pc-windows-gnu" "sipress-windows-x86_64.exe"
        ;;
    windows-native)
        build_native
        ;;
    macos-x86)
        ensure_zig; ensure_zigbuild
        rustup target add x86_64-apple-darwin 2>/dev/null || true
        build_cross "x86_64-apple-darwin" "sipress-macos-x86_64"
        ;;
    macos-arm64)
        ensure_zig; ensure_zigbuild
        rustup target add aarch64-apple-darwin 2>/dev/null || true
        build_cross "aarch64-apple-darwin" "sipress-macos-arm64"
        ;;
    gui)
        build_gui
        ;;
    all)
        ensure_zig; ensure_zigbuild
        rustup target add x86_64-unknown-linux-musl  2>/dev/null || true
        rustup target add aarch64-unknown-linux-musl 2>/dev/null || true
        rustup target add x86_64-pc-windows-gnu      2>/dev/null || true
        build_cross "x86_64-unknown-linux-musl"  "sipress-linux-x86_64"
        build_cross "aarch64-unknown-linux-musl" "sipress-linux-arm64"
        build_cross "x86_64-pc-windows-gnu"      "sipress-windows-x86_64.exe"
        build_native
        build_gui
        ;;
    *)
        echo "Usage: $0 [TARGET]"
        echo ""
        echo "  CLI:"
        echo "    linux-x86        Linux x86_64 static (musl)"
        echo "    linux-arm64      Linux ARM64  static (musl)"
        echo "    windows          Windows x86_64 GNU (zigbuild)"
        echo "    windows-native   Windows x86_64 MSVC (host)"
        echo "    macos-x86        macOS x86_64"
        echo "    macos-arm64      macOS ARM64"
        echo ""
        echo "  GUI:"
        echo "    gui              Tauri GUI — installer + portable (requires node/npm)"
        echo ""
        echo "  Combined:"
        echo "    all              all CLI targets + GUI  [default]"
        exit 1
        ;;
esac

info "Output dir: $OUT/"
ls -lh "$OUT"/ 2>/dev/null || true
