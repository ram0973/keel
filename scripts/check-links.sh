#!/bin/sh
# Проверить ссылки в документации Keel.
#
#   sh scripts/check-links.sh        # только docs/links.md
#   sh scripts/check-links.sh --all  # все .md в docs/
#
# Требует сеть. Битые ссылки — повод поправить docs/links.md.

set -e
cd "$(dirname "$0")/.."

echo "=== проверка ссылок ==="
if python3 scripts/check-links.py "$@"; then
    echo "все ссылки живы"
else
    echo "есть битые ссылки — см. список выше" >&2
    exit 1
fi
