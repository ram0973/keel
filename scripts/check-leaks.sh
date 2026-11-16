#!/bin/bash
# Аудит утечек памяти: собирает каждый пример и play-файл с включённой
# проверкой (KEEL_LEAK_CHECK=1) и печатает, кто протекает.
#
# Смысл: утечки в Keel есть и они задокументированы (буферы read_line
# и read не освобождаются — до появления типа String с владением).
# Этот скрипт делает их видимыми и не даёт им размножиться незаметно.
cd "$(dirname "$0")/.."
KEELC=./target/debug/keelc
if [ ! -x "$KEELC" ]; then
  echo "сначала соберите: cargo build" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

leaks=0
total=0
for f in examples/*.keel play/*.keel; do
  case "$(basename "$f")" in
    users-manual.keel) continue ;;  # черновик
  esac
  total=$((total + 1))
  if ! $KEELC build "$f" "$tmp/prog.c" >/dev/null 2>&1; then
    continue
  fi
  if ! cc -o "$tmp/prog.bin" "$tmp/prog.c" 2>/dev/null; then
    continue
  fi
  out=$(echo "" | KEEL_LEAK_CHECK=1 "$tmp/prog.bin" 2>&1 >/dev/null | grep "утечка")
  if [ -n "$out" ]; then
    echo "УТЕЧКА  $f  →  ${out#keel: }"
    leaks=$((leaks + 1))
  fi
done

echo "проверено: $total, с утечками: $leaks"
