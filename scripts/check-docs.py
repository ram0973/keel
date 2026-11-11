#!/usr/bin/env python3
"""Проверка ссылок между документами: файлы и номера пунктов."""
import os, re, glob

ROOT = os.path.expanduser("~/keel")
docs = glob.glob(os.path.join(ROOT, "docs", "*.md")) + \
    [os.path.join(ROOT, "README.md")]

# Ссылки на файлы других проектов (упомянутые при разборе Zeta и Cranelift)
EXTERNAL = {
    "docs/SELF_COMPILATION_PLAN.md",   # файл в репозитории Zeta
    "docs/ir.md",                       # файл в репозитории wasmtime
}

# 1. Ссылки на файлы
print("=== ссылки на файлы ===")
bad = 0
for d in docs:
    txt = open(d, encoding="utf-8").read()
    for ref in set(re.findall(r"docs/[A-Za-z0-9_.-]+\.md", txt)):
        if ref in EXTERNAL:
            continue
        target = os.path.join(ROOT, ref)
        if not os.path.exists(target):
            print(f"  БИТАЯ: {os.path.basename(d)} -> {ref}")
            bad += 1
print("  все ссылки живые" if not bad else f"  битых: {bad}")

# 2. Ссылки на пункты вида "п. 3.6", "раздел 7.2", "пункт 10"
print("\n=== ссылки на пункты ===")
lang = open(os.path.join(ROOT, "docs/lang-design.md"), encoding="utf-8").read()

# Что есть в lang-design: разделы "## N." и пункты "N. **" внутри раздела 3
sections = set(re.findall(r"^## (\d+)\.", lang, re.M))
items3 = set(re.findall(r"^(\d+)\. \*\*", lang, re.M))
print(f"  разделы в lang-design: {sorted(sections, key=int)}")
print(f"  пункты в разделе 3: {sorted(items3, key=int)}")

bad = 0
for d in docs:
    txt = open(d, encoding="utf-8").read()
    for m in re.finditer(r"п\. (\d+)\.(\d+)", txt):
        sec, item = m.group(1), m.group(2)
        if sec == "3" and item not in items3:
            print(f"  БИТАЯ: {os.path.basename(d)} -> п. {sec}.{item} (нет такого пункта)")
            bad += 1
    for m in re.finditer(r"раздел[а]? (\d+)\.(\d+)", txt):
        if m.group(1) == "3" and m.group(2) not in items3:
            print(f"  БИТАЯ: {os.path.basename(d)} -> раздел {m.group(1)}.{m.group(2)}")
            bad += 1
    for m in re.finditer(r"пункт (\d+)", txt):
        if m.group(1) not in {str(i) for i in range(1, 20)}:
            print(f"  БИТАЯ: {os.path.basename(d)} -> пункт {m.group(1)}")
            bad += 1
print("  все пункты существуют" if not bad else f"  битых: {bad}")

# 3. Внешние символы
print("\n=== посторонние символы ===")
bad = 0
for d in docs:
    txt = open(d, encoding="utf-8").read()
    for i, line in enumerate(txt.splitlines(), 1):
        if re.search(r"[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef]", line):
            print(f"  {os.path.basename(d)}:{i}: {line.strip()[:80]}")
            bad += 1
print("  чисто" if not bad else f"  найдено: {bad}")
