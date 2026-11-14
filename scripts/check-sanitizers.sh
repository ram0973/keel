#!/bin/sh
# Прогон всех примеров под ASan + UBSan + LSan.
#
# Сгенерированный C собирается с санитайзерами и запускается: любая
# утечка, выход за границы или UB — ошибка. Это ловит то, что обычный
# прогон (`cc -w -O2`) пропускает.
set -e
cd "$(dirname "$0")/.."

cargo build --quiet
KC=./target/debug/keelc
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

# Черновики пользователя — не наши, проверять их не нужно.
SKIP="users-manual.keel"
fail=0

for f in examples/*.keel play/*.keel; do
    base=$(basename "$f")
    case " $SKIP " in
        *" $base "*) continue ;;
    esac
    # Модуль без точки входа (user.keel) запускать нечего.
    grep -q "^fn main" "$f" || continue

    name=$(echo "$f" | sed 's|/|_|g; s|\.keel$||')
    printf '%-32s ' "$f"

    if ! "$KC" build "$f" "$OUT/$name.c" >/dev/null 2>&1; then
        echo "ОШИБКА (keel)"
        fail=1
        continue
    fi
    if ! cc -fsanitize=address,undefined -fno-omit-frame-pointer -g -O1 -w \
        -o "$OUT/$name.bin" "$OUT/$name.c" 2>"$OUT/$name.cc.err"; then
        echo "ОШИБКА (cc)"
        head -3 "$OUT/$name.cc.err"
        fail=1
        continue
    fi
    # stdin из /dev/null: примеры с read_line иначе ждали бы терминал.
    if ASAN_OPTIONS=detect_leaks=1:abort_on_error=1 \
       UBSAN_OPTIONS=halt_on_error=1:print_stacktrace=1 \
       timeout 10 "$OUT/$name.bin" </dev/null >/dev/null 2>"$OUT/$name.err"; then
        echo OK
    else
        echo "ОШИБКА (sanitizer)"
        head -6 "$OUT/$name.err"
        fail=1
    fi
done

exit $fail
