#!/usr/bin/env python3
"""Сравнение Keel с Rust: скорость кода и размер бинарника.

Программа берётся из examples/bench.keel, компилируется Keel (Keel -> C ->
cc) и, для контроля, тем же C через clang. Плюс тот же бенчмарк на Rust.

Запуск: python3 scripts/bench.py
"""
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def best_of(cmd, n=7):
    """Лучшее из n прогонов, в миллисекундах."""
    ts = []
    for _ in range(n):
        t0 = time.perf_counter_ns()
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ts.append((time.perf_counter_ns() - t0) / 1e6)
    return min(ts)


def run(cmd, **kw):
    return subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)


def main():
    keel = os.path.join(ROOT, "target", "release", "keelc")
    if not os.path.exists(keel):
        print("сначала: cargo build --release", file=sys.stderr)
        return 1

    c_file = "/tmp/keel_bench.c"
    run([keel, "build", os.path.join(ROOT, "examples", "bench.keel"), c_file])

    print("=== стоимость компиляции C ===")
    for opt in ["-O0", "-O2"]:
        t = best_of(["cc", "-w", opt, "-o", "/tmp/keel_bench_bin", c_file])
        print(f"  cc {opt:4} : {t:7.1f} мс")

    print()
    print("=== скорость программы ===")
    results = {}
    for cc in ["gcc", "clang"]:
        if shutil.which(cc) is None:
            print(f"  {cc:6} : не установлен")
            continue
        out = f"/tmp/keel_bench_{cc}"
        run([cc, "-w", "-O2", "-o", out, c_file])
        results[cc] = best_of([out])
        size = os.path.getsize(out)
        print(f"  Keel->C->{cc:6} : {results[cc]:7.1f} мс   ({size} байт)")

    rust_bin = "/tmp/keel_bench_rust"
    run(["rustc", "-O", "-o", rust_bin, os.path.join(HERE, "bench_rust.rs")])
    results["rust"] = best_of([rust_bin])
    print(f"  Rust -O            : {results['rust']:7.1f} мс   "
          f"({os.path.getsize(rust_bin)} байт)")

    print()
    if "clang" in results:
        print(f"  Keel к Rust : {results['clang'] / results['rust']:.2f}x "
              f"(один и тот же C, clang против LLVM)")
    if "gcc" in results:
        print(f"  gcc против clang на том же C : "
              f"{results['gcc'] / results.get('clang', results['gcc']):.2f}x")
    return 0


if __name__ == "__main__":
    sys.exit(main())
