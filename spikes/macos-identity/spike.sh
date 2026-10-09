#!/bin/sh
# Usage, from the spike directory: ./spike.sh <command> [arguments]
#   identity-create         key, self-signed certificate, temporary keychain, search list entry
#   identity-search-remove  restore the keychain search list that identity-create saved
#   identity-delete         delete the temporary keychain and the key material
#   build                   build the binary three times (v1, v2, v3)
#   bundles                 assemble and sign the six bundles (adhoc and selfsigned, v1..v3)
#   requirements            print the signature facts and designated requirement of each bundle
#   install <variant> <n>   put version n at the install path the way a self-update would
#   launch <variant> <mode> [--secret s] [--service s]
#                           start the installed bundle through LaunchServices and print its result
#   await <result file>     wait again for a result that launch reported as PENDING
#   quarantine <variant>    make a quarantined copy of v1 and open it
#   tcc                     print the permission records macOS holds for both bundle identifiers
#   uninstall               delete the installed bundles
# variant: adhoc | selfsigned. Environment: WAIT_SECONDS (default 10).
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=$HERE/out
SIGNING=$OUT/signing
KEYCHAIN=$SIGNING/junto-spike.keychain-db
CERTIFICATE_NAME="Junto Spike Code Signing"
INSTALL_DIR=$HOME/Applications/JuntoSpike
WAIT_SECONDS=${WAIT_SECONDS:-10}
VERSIONS="1 2 3"

bundle_id() { echo "junto.spike.identity.$1"; }

app_name() {
    case $1 in
        adhoc) echo JuntoSpikeAdhoc ;;
        selfsigned) echo JuntoSpikeSelfSigned ;;
        *) echo "unknown variant $1" >&2; exit 2 ;;
    esac
}

built_app() { echo "$OUT/bundles/$1/v$2/$(app_name "$1").app"; }
installed_app() { echo "$INSTALL_DIR/$(app_name "$1").app"; }

without_home() { sed "s|$HOME|~|g"; }

search_list() { security list-keychains -d user | sed -e 's/^ *"//' -e 's/"$//'; }

set_search_list() {
    list_file=$1
    shift
    extra_count=$#
    while IFS= read -r entry; do
        set -- "$@" "$entry"
    done < "$list_file"
    while [ "$extra_count" -gt 0 ]; do
        extra=$1
        shift
        set -- "$@" "$extra"
        extra_count=$((extra_count - 1))
    done
    security list-keychains -d user -s "$@"
}

identity_create() {
    mkdir -p "$SIGNING"
    chmod 700 "$SIGNING"
    openssl rand -hex 16 > "$SIGNING/keychain-password"
    chmod 600 "$SIGNING/keychain-password"
    password=$(cat "$SIGNING/keychain-password")
    openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
        -keyout "$SIGNING/key.pem" -out "$SIGNING/certificate.pem" \
        -subj "/CN=$CERTIFICATE_NAME" \
        -addext "basicConstraints=critical,CA:FALSE" \
        -addext "keyUsage=critical,digitalSignature" \
        -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null
    security create-keychain -p "$password" "$KEYCHAIN"
    security set-keychain-settings "$KEYCHAIN"
    security unlock-keychain -p "$password" "$KEYCHAIN"
    security import "$SIGNING/key.pem" -k "$KEYCHAIN" -T /usr/bin/codesign
    security import "$SIGNING/certificate.pem" -k "$KEYCHAIN"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$KEYCHAIN" >/dev/null
    search_list > "$SIGNING/search-list.before"
    set_search_list "$SIGNING/search-list.before" "$KEYCHAIN"
    echo "certificate SHA-1: $(openssl x509 -in "$SIGNING/certificate.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')"
    echo "search list now:"
    security list-keychains -d user | without_home
    echo "identities in the temporary keychain (all, then valid only):"
    security find-identity -p codesigning "$KEYCHAIN" | without_home
}

identity_search_remove() {
    set_search_list "$SIGNING/search-list.before"
    echo "search list now:"
    security list-keychains -d user | without_home
}

identity_delete() {
    if [ -f "$KEYCHAIN" ]; then
        security delete-keychain "$KEYCHAIN"
    fi
    rm -rf "$SIGNING"
    echo "search list now:"
    security list-keychains -d user | without_home
}

build() {
    mkdir -p "$OUT/bin"
    for n in $VERSIONS; do
        (cd "$HERE" && SPIKE_VERSION="$n.0.0" ../../tools/serial.sh cargo build --release)
        cp "$HERE/target/release/spike" "$OUT/bin/spike-v$n"
        echo "v$n sha256 $(shasum -a 256 "$OUT/bin/spike-v$n" | cut -d' ' -f1) prints $("$OUT/bin/spike-v$n" version | sed 's/.*"version":"\([^"]*\)".*/\1/')"
    done
}

write_info_plist() {
    variant=$1
    n=$2
    name=$(app_name "$variant")
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>$(bundle_id "$variant")</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundleDisplayName</key><string>$name</string>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$n.0.0</string>
    <key>CFBundleVersion</key><string>$n</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSUIElement</key><true/>
    <key>NSMicrophoneUsageDescription</key><string>Identity spike: asks for the microphone permission only. Nothing is recorded.</string>
    <key>NSAudioCaptureUsageDescription</key><string>Identity spike: asks for the system audio permission only. Nothing is recorded.</string>
</dict>
</plist>
PLIST
}

bundles() {
    rm -rf "$OUT/bundles"
    for variant in adhoc selfsigned; do
        for n in $VERSIONS; do
            app=$(built_app "$variant" "$n")
            name=$(app_name "$variant")
            mkdir -p "$app/Contents/MacOS"
            cp "$OUT/bin/spike-v$n" "$app/Contents/MacOS/$name"
            write_info_plist "$variant" "$n" > "$app/Contents/Info.plist"
            if [ "$variant" = adhoc ]; then
                codesign --force --sign - "$app"
            else
                codesign --force --timestamp=none --keychain "$KEYCHAIN" --sign "$CERTIFICATE_NAME" "$app"
            fi
        done
    done
    requirements
}

requirements() {
    for variant in adhoc selfsigned; do
        for n in $VERSIONS; do
            app=$(built_app "$variant" "$n")
            echo "--- $variant v$n"
            codesign -d -vvv "$app" 2>&1 | grep -E '^(Identifier|CDHash|Signature|Authority|TeamIdentifier|CodeDirectory)' | sed 's/^/  /'
            codesign -d -r- "$app" 2>&1 | grep -E '^(designated|# designated)' | sed 's/^/  /'
            printf '  verify: '
            codesign --verify --strict --verbose=2 "$app" 2>&1 | without_home | tr '\n' ';'
            echo
        done
    done
}

install() {
    variant=$1
    n=$2
    name=$(app_name "$variant")
    destination=$(installed_app "$variant")
    staged=$INSTALL_DIR/.$name.staged.app
    previous=$INSTALL_DIR/.$name.previous.app
    mkdir -p "$INSTALL_DIR"
    rm -rf "$staged" "$previous"
    ditto "$(built_app "$variant" "$n")" "$staged"
    if [ -d "$destination" ]; then
        mv "$destination" "$previous"
    fi
    mv "$staged" "$destination"
    rm -rf "$previous"
    echo "installed $variant v$n at $(echo "$destination" | without_home)"
    codesign -d -r- "$destination" 2>&1 | grep -E '^(designated|# designated)' | sed 's/^/  /'
    printf '  verify: '
    codesign --verify --strict --verbose=2 "$destination" 2>&1 | without_home | tr '\n' ';'
    echo
}

await() {
    result=$1
    waited=0
    while [ ! -f "$result" ] && [ "$waited" -lt "$WAIT_SECONDS" ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if [ -f "$result" ]; then
        cat "$result"
    else
        echo "PENDING $result"
    fi
}

launch() {
    variant=$1
    mode=$2
    shift 2
    mkdir -p "$OUT/results"
    result=$OUT/results/$(date +%H%M%S)-$variant-$mode.json
    open -n "$(installed_app "$variant")" --args "$mode" "$@" --out "$result"
    await "$result"
}

quarantine() {
    variant=$1
    name=$(app_name "$variant")
    copy=$INSTALL_DIR/Downloaded/$name.app
    rm -rf "$copy"
    mkdir -p "$INSTALL_DIR/Downloaded"
    ditto "$(built_app "$variant" 1)" "$copy"
    stamp=$(printf '%x' "$(date +%s)")
    xattr -r -w com.apple.quarantine "0083;$stamp;Safari;$(uuidgen)" "$copy"
    echo "quarantine attribute: $(xattr -p com.apple.quarantine "$copy")"
    printf 'spctl: '
    spctl --assess --type execute --verbose=4 "$copy" 2>&1 | without_home | tr '\n' ';'
    echo
    mkdir -p "$OUT/results"
    result=$OUT/results/$(date +%H%M%S)-$variant-quarantined.json
    open -n "$copy" --args version --out "$result" 2>&1 | without_home || true
    await "$result"
}

tcc_rows() {
    database=$1
    sqlite3 -separator '|' "$database" \
        "select service, client, auth_value, auth_reason, hex(csreq), datetime(last_modified, 'unixepoch', 'localtime') from access where client like 'junto.spike.identity.%' order by client, service" 2>&1 |
    while IFS='|' read -r service client auth_value auth_reason requirement modified; do
        decoded=$(printf '%s' "$requirement" | xxd -r -p | csreq -r- -t 2>/dev/null || true)
        echo "  $service $client auth_value=$auth_value auth_reason=$auth_reason modified=$modified requirement: $decoded"
    done
}

tcc() {
    echo "user permission database:"
    tcc_rows "$HOME/Library/Application Support/com.apple.TCC/TCC.db"
    echo "system permission database:"
    tcc_rows "/Library/Application Support/com.apple.TCC/TCC.db"
}

uninstall() {
    rm -rf "$INSTALL_DIR"
    echo "install directory exists: $([ -e "$INSTALL_DIR" ] && echo yes || echo no)"
}

command=${1:-}
[ -n "$command" ] || { sed -n '2,16p' "$0"; exit 2; }
shift
case $command in
    identity-create) identity_create ;;
    identity-search-remove) identity_search_remove ;;
    identity-delete) identity_delete ;;
    build) build ;;
    bundles) bundles ;;
    requirements) requirements ;;
    install) install "$@" ;;
    launch) launch "$@" ;;
    await) await "$@" ;;
    quarantine) quarantine "$@" ;;
    tcc) tcc ;;
    uninstall) uninstall ;;
    *) echo "unknown command $command" >&2; exit 2 ;;
esac
