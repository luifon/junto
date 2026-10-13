#!/bin/sh
set -eu
case ${JUNTO_CODESIGN_IDENTITY:-} in
    ''|-) echo 'a code-signing certificate is required; ad-hoc signatures are refused' >&2; exit 2 ;;
esac
: "${JUNTO_CODESIGN_KEYCHAIN:?set the dedicated code-signing keychain path}"
if [ "$#" -ne 1 ]; then
    echo 'usage: packaging/sign.sh <Junto.app>' >&2
    exit 2
fi
exec python3 "$(dirname "$0")/sign.py" "$1"
