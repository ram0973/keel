#!/bin/sh
# Прогон всех примеров: каждый должен скомпилироваться и отработать.
set -e
cd "$(dirname "$0")/.."
for f in examples/*.keel; do
    printf '%-32s ' "$f"
    if cargo run --quiet -- run "$f" >/dev/null 2>&1; then
        echo OK
    else
        echo ОШИБКА
        cargo run --quiet -- check "$f" || true
    fi
done
