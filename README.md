# Keel

Экспериментальный язык: **синтаксис в духе Rust** (`fn`, `let`, `impl`,
`&T`), безопасность памяти на этапе компиляции, быстрая компиляция.
Цель — проверить, работает ли на практике модель group borrowing
(paths + use-after-free вместо shared-xor-mutable).

От C в синтаксисе только `{ }`, `;` и `//`-комментарии; всё остальное
взято из Rust. Отличия от Rust — тип после имени (`x int`), один вид
ссылки `&T` без `&mut`, нет lifetime-аннотаций, изменяемость через
`mut`-контракты.

Компилятор написан на Rust, бэкенд генерирует C.

## Быстрый старт

### 1. Клонировать

```sh
git clone https://github.com/ram0973/keel.git
cd keel
```

### 2. Установить зависимости

Нужны три вещи:

| Что | Зачем | Проверить |
|---|---|---|
| **Rust** (rustc + cargo) | сборка компилятора | `cargo --version` |
| **C-компилятор** | сборка сгенерированного C | `cc --version` |
| **git** | клонирование | `git --version` |

Если Rust не установлен:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

C-компилятор (`cc`, обычно это gcc или clang):

```sh
# Debian / Ubuntu
sudo apt install build-essential

# Fedora
sudo dnf install gcc
```

Проверено на: `rustc 1.99.1`, `cc (Ubuntu 15.2.0)`.

### 3. Собрать компилятор

```sh
cargo build
```

Бинарник появится в `target/debug/keelc`. Дальше вызываем его напрямую —
это быстрее, чем `cargo run`.

### 4. Запустить свой пример

Положите файл `myprogram.keel` в корень проекта и выполните:

```sh
# только проверить (без сборки C — быстро)
./target/debug/keelc check myprogram.keel

# собрать и запустить
./target/debug/keelc run myprogram.keel

# собрать готовый исполняемый файл
./target/debug/keelc compile myprogram.keel myprogram
./myprogram
```

`compile` оставляет на диске два файла: сам бинарник и рядом
`myprogram.c` — сгенерированный C, который можно прочитать, отладить
в `gdb` или собрать вручную:

```sh
cc -O2 -g myprogram.c -o myprogram
./myprogram
```

### 5. Запустить примеры из репозитория

```sh
# программа-документация: десять разделов, каждый объясняет одну возможность
./target/debug/keelc run examples/language-tour.keel

# методы и impl-блоки
./target/debug/keelc run examples/methods.keel

# модули: три файла, import, плоские имена
./target/debug/keelc run examples/modules.keel

# что означает & и когда значение перемещается
./target/debug/keelc run examples/references.keel

# телефонная книга: CRUD и инвалидация ссылки
./target/debug/keelc run examples/phonebook.keel

# мир с существами: параметр-внутри, group borrowing, сортировка
./target/debug/keelc run examples/mini.keel

# что нового в волне 7: len(), const, печать нескольких значений,
# конкатенация строк
./target/debug/keelc run examples/wave7.keel
./target/debug/keelc run examples/wave7-io.keel один два   # ввод и argv
```

Прогнать все примеры и тесты сразу:

```sh
cargo test
sh scripts/check-examples.sh
```

### 6. Туториал `play/`

Папка `play/` — **туториал по языку**: от переменных до владения, каждая
часть — рабочая программа. Она **входит в репозиторий** и проверяется
вместе с примерами (`scripts/check-examples.sh`, блок «туториал play/»).

```sh
./target/debug/keelc run play/main.keel     # точка входа
```

Файлы идут по возрастанию сложности:

| # | Файл | О чём |
|---|---|---|
| 1 | `01-basics.keel` | переменные, `let`/`let mut`, три типа, печать, арифметика, `const`, ввод и аргументы |
| 2 | `02-control-flow.keel` | `if`/`else`, логика, `if` как выражение, `while`, `do-while`, `for`, `break`/`continue` |
| 3 | `03-functions-structs.keel` | функции, `return`/`return;`, рекурсия, структуры, массивы, методы, `self`, `mut(self)`, ассоциированные функции |
| 4 | `04-ownership.keel` | **владение, ссылки, пути, `mut`-контракты, group borrowing, анализ выхода** |
| 5 | `05-modules.keel` | модули: файл = модуль, плоские имена |
| 6 | `06-practice.keel` | телефонная книга — всё вместе |
| 7 | `07-errors.keel` | ошибки компилятора и их смысл |

`main.keel` — оглавление-точка входа, `user.keel` — модуль для части 5.

Подробное описание, шпаргалка по синтаксису и список ограничений —
в `play/README.md`. Если читать только один файл — `04-ownership.keel`.

## Состояние

MVP. Реализовано:

- лексер, парсер (рекурсивный спуск), AST;
- проверщик владения и заимствований на путях;
- генерация C; сборка и запуск — через `cc`;
- **146 тестов**, покрывающих семантику модели;
- 15 примеров, включая многофайловый и ввод/аргументы;
- **туториал** из 7 частей в `play/` (от переменных до владения);
- **анализ выхода**: ссылка не может пережить функцию, в которой создана;
- **ввод**: `read_line()`, `read_int()`, `argc()`, `argv(i)`.

Не реализовано (сознательно): обобщённые типы, `Vec`, `Option`/`Result`,
лямбды, трейты, `enum`, `match`, приватность модулей (`pub`),
асинхронность, Higher RAII, generational references.

## Запуск через cargo

Команды те же, но через cargo — удобно, если не хочется помнить путь
к бинарнику. Работает и без предварительного `cargo build`.

```sh
cargo run -- check examples/tour.keel
cargo run -- build examples/tour.keel /tmp/out.c
cargo run -- compile examples/tour.keel /tmp/tour
cargo run -- run examples/tour.keel
cargo test
```

## Запуск демонстрации

**Программа-документация** — `examples/language-tour.keel`.
Десять разделов, каждый показывает одну возможность языка; комментарии
объясняют, что происходит. Это и документация, и проверка компилятора.

Раздел 9 — «Что Keel запрещает и как это решать правильно»: каждый
запрет разобран по схеме **проблема → почему так опасно → решение**,
с точными сообщениями компилятора. В том числе — как объявить пустой
массив: `let xs [int; 0] = [];` (тип элемента обязателен — выводить его
не из чего).

```sh
cd ~/keel
cargo run -- run examples/language-tour.keel
```

**Прикладные примеры:**

| Файл | Что показывает |
|---|---|
| `examples/phonebook.keel` | Телефонная книга: добавление, поиск, правка, удаление. Показывает, как borrow checker ловит устаревшую ссылку на запись |
| `examples/mini.keel` | Мир с существами: параметр-внутри, контракт `mut`, две изменяемые ссылки на элементы одной коллекции, сортировка |
| `examples/tour.keel` | Тот же код, что и в `examples/rust/rust_comparison.rs`, но на Keel: на Rust не компилируется, на Keel — работает |
| `examples/methods.keel` | Методы и `impl`-блоки: ассоциированные функции, `self`, `mut(self)`, методы разных типов с одним именем |
| `examples/references.keel` | Что означает `&`: без него значение перемещается, с ним вызывающий сохраняет владение |
| `examples/self-value.keel` | Зачем нужен `self` без `&`: единственный случай — положить объект в поле другой структуры |
| `examples/modules.keel` | Модули: три файла, `import`, плоские имена. `registry.keel` сам подключает `user.keel` |
| `experiments/clif/` | Эксперимент: та же функция, но собранная в IR Cranelift напрямую. Оценка перехода с C-бэкенда на собственный IR |

```sh
cargo run -- run examples/phonebook.keel
cargo run -- run examples/mini.keel
cargo run -- run examples/methods.keel
cargo run -- run examples/modules.keel
cargo run -- run examples/tour.keel
```

**Проверить все примеры сразу:**

```sh
sh scripts/check-examples.sh
```

**Сравнение с Rust по скорости и размеру:**

```sh
cargo build --release
python3 scripts/bench.py
```

Выведет скорость программы, собранной Keel (через C) против того же
бенчмарка на Rust, и размеры бинарников. По умолчанию сгенерированный C
собирается с `-O2`; переопределить уровень можно переменной
`KEEL_OPT` (например, `KEEL_OPT=-O0 cargo run -- run ...`).

**Из Windows** команды выполняются через WSL:

```sh
wsl -d Ubuntu-26.04 -- bash -lc "cd ~/keel && cargo run -- run examples/language-tour.keel"
```

Остальные команды компилятора:

```sh
# только проверка, без сборки C и запуска
cargo run -- check examples/language-tour.keel

# сгенерировать C и посмотреть на него
cargo run -- build examples/language-tour.keel /tmp/out.c
cat /tmp/out.c
```

**Собрать всё один раз**, чтобы не ждать компиляцию при каждом запуске:

```sh
cargo build
./target/debug/keelc run examples/language-tour.keel
```

## Что проверяется

Главная проверка — сравнение с Rust. Файл `examples/tour.keel` содержит
код, который в Rust не компилируется:

```
error[E0499]: cannot borrow `world.entities` as mutable more than once at a time
error[E0502]: cannot borrow `world.entities` as mutable because it is also borrowed as immutable
```

(`examples/rust_comparison.rs` — тот же код на Rust, для проверки.)

В Keel это обычный код:

```
fn step(world &World, entity in world.entities[] mut, amount int) {
    entity.hp = entity.hp - amount;
}

fn heal_first_and_second(world &World) mut(world.entities[]) {
    let a = &world.entities[0];
    let b = &world.entities[1];
    a.hp = a.hp + 10;
    b.hp = b.hp + 10;
}
```

Проверяются и обратные случаи — то, что должно падать:

- использование ссылки после замены объекта;
- использование значения после move;
- присваивание неизменяемой переменной;
- запись в путь, не объявленный изменяемым;
- функция, меняющая путь, который не объявила в mut-контракте.

## Ключевые находки MVP

Подробно — в `docs/mvp-results.md`. Коротко:

1. **«Право писать» и «область инвалидации» — разные вещи.** `let mut`
   даёт право переприсваивать переменную; `mut(путь)` в сигнатуре — это
   контракт («функция меняет эти пути»), по которому вызывающий
   инвалидирует свои ссылки. Смешивать их в одну проверку нельзя: тогда
   либо слишком строго (запрещается всё), либо теряется инвалидация.

2. **Путь `[]` — шаблон, а не конкретный индекс.** `world.entities[]`
   является префиксом `world.entities[0].hp`. Это позволяет объявлять
   контракт на уровне коллекции, не перечисляя элементы.

3. **Инвалидация бывает двух видов.** Замена значения (`x.f = v`) убивает
   ссылки на сам путь и всё внутри него. Изменение содержимого
   (`vec.push(..)`) убивает только ссылки строго внутри.

4. **Путь параметра — символический** (имя параметра), а не локальный uid.
   Контракт вызываемой функции переносится в пространство вызывающего
   через аргументы. Это то, что делает проверку межпроцедурной.

5. **Проверщик обязан быть детерминированным.** Первая версия искала
   владелец пути по обходу `HashMap` и выдавала разные результаты в
   зависимости от seed. Нашлось только потому, что тесты гонялись в двух
   разных окружениях.

6. **Главный риск проекта не подтвердился.** Ожидалось, что ослабление
   правил Rust потребует более тяжёлого анализа. На практике проверка на
   путях оказалась простой (~900 строк на всю семантику владения). Цена
   переносится в другое место — в точность контракта и в диагностику.

## Документация

- `docs/mvp-results.md` — **результаты MVP и размышления**: что построено,
  что доказано, что пришлось изменить в дизайне, какие ошибки нашлись,
  что не работает и что дальше. Начинать отсюда.
- `docs/lang-design.md` — стратегия, механизмы, правила владения,
  модель компиляции. В разделе 8 — план по возможностям языка: волны,
  сроки и обоснование порядка.
- `docs/keel-syntax.md` — грамматика, открытые вопросы.
- `docs/keel-risks.md` — минусы и подводные камни, с ценой.
- `docs/links.md` — подборка материалов по реализации языков: как написать
  свой компилятор, курсы, статьи по владению и регионам, генераторы
  парсеров. Ссылки проверяются скриптом `scripts/check-links.py`.
- `docs/keel-explained.md` — то же самое простыми словами, для того, кто
  знает только C.
- `docs/cranelift-experiment.md` — **эксперимент с бэкендом на Cranelift**:
  та же функция, собранная в IR напрямую; измеренная цена миграции и
  вывод, что SSA-конструктор писать не придётся.
- `docs/zeta-comparison.md` — **разбор Zeta**: что в ней реально
  реализовано, что только обещано в документации, что берём в Keel и что
  нет (с обоснованием). Там же — решение по синтаксису: `x int` или
  `x: int`.

## Литература

Основа — работы Evan Ovadia и Nick Smith:

- Valen's Memory Safety: A New Kind of Borrow Checking
- Group Borrowing: Zero-Cost Memory Safety with Fewer Restrictions
- A New Way to Think About Borrow Checking (In Reverse)
- Making C++ Memory-Safe Without Borrow Checking, Reference Counting, or
  Tracing Garbage Collection
- Higher RAII, and the Seven Arcane Uses of Linear Types
- The Memory Safety Grimoire
