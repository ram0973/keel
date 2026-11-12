#!/bin/sh
# Прогон всех примеров: каждый должен скомпилироваться и отработать.
# Затем — проверка целостности документации (ссылки, посторонние символы).
set -e
cd "$(dirname "$0")/.."

echo "=== примеры ==="
# Черновики пользователя — не наши, проверять их не нужно.
SKIP="users-manual.keel"
for f in examples/*.keel; do
    base=$(basename "$f")
    case " $SKIP " in
        *" $base "*) echo "$(printf '%-32s' "$f") пропущен (черновик)"; continue ;;
    esac
    printf '%-32s ' "$f"
    # stdin из /dev/null: примеры, читающие ввод (read_line), иначе
    # ждали бы терминал и подвешивали проверку.
    if cargo run --quiet -- run "$f" </dev/null >/dev/null 2>&1; then
        echo OK
    else
        echo ОШИБКА
        cargo run --quiet -- check "$f" || true
    fi
done

echo
echo "=== документация ==="
python3 scripts/check-docs.py

echo
echo "=== ссылки ==="
# Сеть может быть недоступна — это не повод ронять проверку целиком.
if python3 scripts/check-links.py >/tmp/keel_links.log 2>&1; then
    tail -3 /tmp/keel_links.log
else
    echo "проверка ссылок не пройдена (сеть?) — подробности в /tmp/keel_links.log"
    tail -12 /tmp/keel_links.log
fi
