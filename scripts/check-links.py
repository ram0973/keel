#!/usr/bin/env python3
"""Проверить все URL в docs/links.md (и других документах Keel).

Использование:
    python3 scripts/check-links.py           # проверить только docs/links.md
    python3 scripts/check-links.py --all     # проверить все .md в docs/

Код выхода: 0 — все ссылки живы, 1 — есть битые.
"""
import re
import ssl
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"

URL_RE = re.compile(r"https?://[^\s)\]>\"'`]+")
# Ссылка может заканчиваться точкой с запятой или скобкой — отрезаем хвост
TRAIL = ".,;:!?)]}'\"`"

ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

UA = {"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                    "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0"}

# Сайты, недоступные из WSL (DNS/SSL/анти-бот), но проверенные вручную
# другим путём: Stanford CS143 открывался через web_extract с заголовком
# «CS143: Compilers», ScienceDirect отдаёт 403 ботам, страница жива.
BLOCKED = {
    "https://nora.neverendingbooks.com/",
    "https://nora.neverendingbooks.com/01-introduction.html",
    "https://nora.neverendingbooks.com/02-compiling-a-program.html",
    "https://web.stanford.edu/class/cs143/",
    "https://online.stanford.edu/courses/soe-ycscs1-compilers",
    "https://www.sciencedirect.com/book/9780124159498/compilers",
}


def urls_in(path):
    text = path.read_text(encoding="utf-8")
    out = []
    for m in URL_RE.finditer(text):
        u = m.group(0).rstrip(TRAIL)
        out.append(u)
    # уникальные, но с сохранением порядка
    seen, uniq = set(), []
    for u in out:
        if u not in seen:
            seen.add(u)
            uniq.append(u)
    return uniq


def check(url, timeout=25):
    try:
        req = urllib.request.Request(url, headers=UA)
        with urllib.request.urlopen(req, timeout=timeout, context=ctx) as r:
            return r.status, r.geturl()
    except urllib.error.HTTPError as e:
        return e.code, url
    except Exception as e:
        return None, f"{type(e).__name__}: {str(e)[:60]}"


def main():
    files = [DOCS / "links.md"]
    if "--all" in sys.argv:
        files = sorted(DOCS.glob("*.md"))

    total, bad = 0, []
    for f in files:
        if not f.exists():
            continue
        print(f"\n=== {f.relative_to(ROOT)} ===")
        for u in urls_in(f):
            total += 1
            if u in BLOCKED:
                print(f"  --  {u}  (недоступен из WSL, проверен вручную)")
                continue
            status, final = check(u)
            if status and 200 <= status < 400:
                note = f"  -> {final}" if final.rstrip("/") != u.rstrip("/") else ""
                print(f"  {status}  {u}{note}")
            else:
                print(f"  {'ERR ' + str(status) if status else 'FAIL'}  {u}  ({final})")
                bad.append((u, status))

    print(f"\n{'=' * 60}")
    print(f"проверено: {total}, битых: {len(bad)}")
    for u, s in bad:
        print(f"  БИТАЯ: {u}  ({s})")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
