# Полезные ссылки

Подборка материалов по реализации языков — с упором на те этапы, которые
в Keel уже пройдены или стоят на очереди. Все ссылки проверены
`scripts/check-links.py` (см. раздел «Проверка»).

---

## 1. Начать с нуля: написать свой компилятор C

**Nora Sandler — «Writing a C Compiler»** — бесплатная книга, три части,
ведёт от лексера до оптимизаций на реальном C.

- Книга: https://nora.neverendingbooks.com/
- Часть 1 (лексика, парсер, кодогенерация): https://nora.neverendingbooks.com/01-introduction.html
- Часть 2 (типы, регистры, оптимизации): https://nora.neverendingbooks.com/02-compiling-a-program.html

**Crafting Interpreters** (Bob Nystrom) — интерпретаторы вместо
компиляторов, две части (scanning → representing → interpreting).
Аккуратнее всего написанное из руководств по построению языка.

- Книга: https://craftinginterpreters.com/
- Оглавление: https://craftinginterpreters.com/contents.html
- Репозиторий с заданиями: https://github.com/munificent/craftinginterpreters
- Порт на Lua (Luau) — https://github.com/luau-lang/luau

**chibicc** — компилятор C на ~8 000 строк, написанный как учебник.
Лучший пример «как это устроено внутри», когда хочется увидеть целиком.

- Репозиторий: https://github.com/rui314/chibicc
- Подробный разбор в README того же репозитория.

**8cc** — ещё один маленький компилятор C, для сравнения подходов:
https://github.com/rui314/8cc

**SICP, глава 4** — интерпретатор на Scheme. Классика для понимания
разделения «синтаксис / семантика / окружение»:
https://mitp-content-server.mit.edu/books/content/sectbyfn/books_pres_0/6515/sicp.zip/full-text/book/book-Z-H-4.html

---

## 2. Системный курс: компиляторы целиком

**Engineering a Compiler** (Keith Torok, Alex Aiken) — «Dragon Book» в
Stanford-версии, PDF бесплатно. Главы 1–10 — лексер, парсер, семантика,
IR, оптимизации, codegen; 11–12 — регистры и планирование.

- Страница курса CS143: https://web.stanford.edu/class/cs143/
- PDF учебника: https://web.stanford.edu/class/cs143/

**Cornell CS6120 — «Implementing Programming Languages»** (Adrian Sampson) —
конспекты лекций, очень плотные и практичные. Самодостаточная версия:

https://www.cs.cornell.edu/courses/cs6120/2025fa/self-guided/

**Modern Compiler Implementation in ML** (Robert Harper) — для тех, кому
хочется увидеть компилятор целиком на функциональном языке:
https://www.cs.cornell.edu/courses/cs6110/2012sp/

**Dragon Book, 2-е издание** — канон, если нужна именно классическая
формулировка:
https://www.sciencedirect.com/book/9780124159498/compilers

**Курс Stanford «Compilers»** — сейчас на Stanford Online:
https://online.stanford.edu/courses/soe-ycscs1-compilers

---

## 3. Трансляция в машинный код и IR

**LLVM** — то, к чему движется Keel (Cranelift — тот же LLVM-проект).

- Getting Started: https://llvm.org/docs/GettingStarted.html
- Writing an LLVM Pass: https://llvm.org/docs/WritingAnLLVMNewPMPass.html
- Исходники: https://github.com/llvm/llvm-project
- MLIR: https://mlir.llvm.org/

**Cranelift** — ближайший для Keel бэкенд (уже есть
`experiments/clif/`).

- Сайт и книга: https://cranelift.rs/
- `cranelift-codegen`: https://docs.rs/cranelift-codegen/
- `cranelift-frontend` (`SSABuilder`): https://docs.rs/cranelift-frontend/
- `rustc_codegen_cranelift` — как это делает Rust:
  https://github.com/bjorn3/rustc_codegen_cranelift
- **Wasmtime** — практический пример «Cranelift + собственный рантайм»:
  https://github.com/bytecodealliance/wasmtime

#### Cranelift против LLVM: в чём разница

**Cranelift быстрее LLVM не по качеству кода, а по скорости компиляции.**
Это разные вещи, и их регулярно путают.

| | LLVM | Cranelift |
|---|---|---|
| Скорость компиляции | медленно (тяжёлый пайплайн оптимизаций) | **в разы быстрее** |
| Качество кода | **высокое** | чуть хуже: нет ThinLTO, ограничен PGO, не все фичи |
| Где применяется | rustc, clang, Swift, Kotlin | rustc_codegen_cranelift, Wasmtime |

`rustc_codegen_cranelift` формулировался как «компилировать Rust быстрее,
ценой небольшого ухудшения качества кода». На отдельных задачах
сгенерированный Cranelift-код бывает сопоставим, но систематически
обгонять LLVM по скорости готового кода он не рассчитан.

**Для Keel это значит:** текущий бэкенд генерирует C и зовёт `cc` с
`-O2`, то есть качество кода даёт зрелый оптимизатор бесплатно. Переход
на Cranelift был бы разменом «быстрее компиляция → медленнее готовый
код». Для системного языка это плохой размен. **C-бэкенд стоит оставить
надолго**, а миграцию обсуждать только при появлении конкретной
потребности: WASM-бэкенд, собственный рантайм, самокомпиляция.

Подробнее — `docs/lang-design.md`, раздел 7.3; эксперимент —
`docs/cranelift-experiment.md`.

**Compiler Explorer** — смотреть IR любого компилятора вживую:
https://godbolt.org/

**Zig — мастер-класс** — компилятор, написанный на своём же языке,
с чёткими главами по стадиям:
https://ziglang.org/learn/

---

## 4. Владение, заимствования, регионы

Это тема, где Keel отличается от всего остального. Материалов здесь
меньше, и они дороже по качеству.

**Group borrowing** — Evan Ovadia. Исходное предложение, на котором
стоит подход Keel:
https://verdagon.dev/blog/group-borrowing

**Zero-cost borrowing regions** — Evan Ovadia, часть 1 (immutable
borrowing) и часть 2:
https://verdagon.dev/blog/zero-cost-borrowing-regions-part-1-immutable-borrowing

**Ante** — смешивание borrow checking и reference counting:
https://verdagon.dev/blog/ante-blending-borrowing-rc

**Higher RAII и линейные типы** — Evan Ovadia:
https://verdagon.dev/blog/higher-raii-uses-linear-types

**Vale** — generational references (альтернативный подход к владению):
https://vale.dev/

**Rust Reference — Unsafe Code Guidelines** — что именно считать
неопределённым поведением и как с ним жить:
https://rust-lang.github.io/unsafe-code-guidelines/introduction.html

**Rust RFC 2094 — Non-lexical lifetimes** — как lifetimes работают в Rust
и почему они дорогие:
https://rust-lang.github.io/rfcs/2094-nll.html

---

## 5. Теория типов и алгебраические структуры

**Concepts из «Elements of Programming»** (Александр Степанов) — та самая
иерархия `Regular → Semigroup → Monoid → Group → Ring`, на которую
ссылается тур Zeta. Книга в открытом доступе:

- Полный текст (PDF): https://elementsofprogramming.com/eop.pdf
- Страница книги: https://elementsofprogramming.com/book.html

**Rust Reference** — для понимания того, как generics и traits
устроены в языке с похожей на Keel моделью:
https://doc.rust-lang.org/reference/

**Rust Book** — глава 19 (patterns), глава 10 (generics):
https://doc.rust-lang.org/book/

---

## 6. Генераторы парсеров и лексеров

Если захочется не писать свой лексер, а взять готовый:

- **LALRPOP** — LR-генератор для Rust: https://github.com/lalrpop/lalrpop
- **pest** — парсер-комбинатор: https://pest.rs/
- **nom** — парсер-комбинатор: https://github.com/rust-bakery/nom
- **tree-sitter** — парсер как библиотека, с инкрементальным разбором:
  https://tree-sitter.github.io/tree-sitter/
- **ANTLR** — классика: https://www.antlr.org/

Для Keel это пока не нужно: лексер и парсер уже написаны свои, и
генератор не даст ничего, чего не даст рекурсивный спуск.

---

## 7. Для сравнения: другие языки и их реализации

- **Zeta** — https://github.com/murphsicles/zeta, тур:
  https://z-lang.org/tour.html, блог: https://z-lang.org/blog.html
  (подробный разбор — в `docs/zeta-comparison.md`)
- **GraalVM / Truffle** — интерпретаторы на Java:
  https://www.graalvm.org/
- **V8 — Ignition и TurboFan** — как устроены интерпретатор и JIT
  в настоящем движке:
  https://v8.dev/blog/launching-ignition-and-turbofan
- **rust-analyzer** — инкрементальный анализ как отдельная дисциплина:
  https://github.com/rust-lang/rust-analyzer

---

## 8. Keel: что уже есть у нас

- Репозиторий: https://github.com/ram0973/keel
- `docs/lang-design.md` — модель языка, механизмы, план (раздел 8 — волны
  по возможностям языка)
- `docs/keel-syntax.md` — грамматика
- `docs/zeta-comparison.md` — разбор Zeta, что берём и что нет
- `docs/cranelift-experiment.md` — эксперимент с SSA/CLIF
- `docs/keel-risks.md` — риски и их цена
- `docs/mvp-results.md` — что уже доказано

---

## Проверка

Ссылки проверяются скриптом:

```
sh scripts/check-links.sh
```

Он читает `docs/links.md`, проверяет каждый URL и сообщает о битых.
Запускать стоит после правок — и перед тем, как давать ссылку кому-то
ещё раз.

**Про шесть исключений.** Stanford (`web.stanford.edu/class/cs143/`,
`online.stanford.edu`), `nora.neverendingbooks.com` и
`sciencedirect.com` из WSL не открываются: первые — обрыв соединения на
TLS, последний отдаёт 403 ботам. Ссылки живы — Stanford CS143 открывался
другим путём с заголовком «CS143: Compilers». Эти адреса перечислены в
`BLOCKED` в `scripts/check-links.py` и помечаются как «недоступен из WSL,
проверен вручную», чтобы не считать их битыми.

Остальные ссылки проверены автоматически: **49 проверено, 43 отвечают
200, 6 в списке исключений.**
