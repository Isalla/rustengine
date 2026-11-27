#!/usr/bin/env sh
# ADR-0004 guard: no phone-home dependencies.
#
# Engine binaries must never carry HTTP/TLS/websocket/analytics client
# crates into the runtime dependency graph — the engine only ever talks
# to endpoints a human explicitly supplied. Run from the repo root; CI
# runs it on every push. A future game-transport crate (M6+) needs an
# ADR-0004 amendment first, per its rule 5.
set -eu
cd "$(dirname "$0")/.."

FORBIDDEN='reqwest|ureq|attohttpc|simple_http|minreq|isahc|awc|hyper|tungstenite|websock|sentry|crash_handler|crash-handler|hyper-tls|native-tls|rustls|openssl'

HITS=$(cargo tree -e normal --workspace 2>/dev/null | grep -Ei "$FORBIDDEN" || true)
if [ -n "$HITS" ]; then
    echo "ADR-0004 violation: phone-home-capable crates in the dependency graph:" >&2
    echo "$HITS" >&2
    exit 1
fi
echo "ok: no phone-home dependencies (ADR-0004)"
