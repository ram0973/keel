# Cranelift-бэкенд: план и проверенные факты

Замысел: `keelc clif <file.keel>` собирает исполняемый файл через Cranelift
(IR → машинный код → объектный файл → `cc` для линковки с рантаймом),
минуя генерацию C.

## Что уже проверено (эксперимент `experiments/clif`)

- Кодогенерация Cranelift — около 2 мс, объектный файл ~830 байт;
  `cc` на том же коде — около 30 мс.
- API Cranelift 0.137.1 (крейты лежат в локальном реестре, сборка `--offline`):
  - `cranelift_object::ObjectBuilder::new(isa, "имя.o", default_libcall_names())`
    → `ObjectModule::new(builder)`;
  - `module.declare_function(name, Linkage::Export, &sig)`;
  - `module.make_context()` → `ctx.func.signature = sig`;
  - `FunctionBuilder::new(&mut ctx.func, &mut fb_ctx)`;
  - `module.define_function(id, &mut ctx)`;
  - `module.finish().emit()` → `Vec<u8>` — готовый объектный файл.
- `cranelift_frontend::FunctionBuilder` даёт автоматический SSA:
  `declare_var` → `def_var` → `use_var`, затем `b.finalize(target_config)`.
- `b.is_unreachable()` есть, но надёжнее вести свой флаг «блок завершён».

## Объём MVP (подмножество Keel)

Поддерживается:

- функции с параметрами и возвратом `int`, `float`, `bool`, `string`;
- `let`, присваивание переменной, `return`;
- арифметика, сравнения, логика `&&`/`||`/`!` (с коротким замыканием);
- `if`/`else` (оператор и выражение), `while`, `do-while`,
  `for i in a..b` (включая `..=` и `step`);
- `break`, `continue`;
- вызовы пользовательских функций;
- `print`/`println` для `int`, `float`, `bool`, `string`.

Не поддерживается (понятная ошибка, а не падение):
структуры, enum, `impl` и методы, `Option`/`Result`, `Vec`, `Box`, кортежи,
массивы, срезы, ссылки, `match`, замыкания, `pre`/`post`, `import`.

## Раскладка по слоям

- `src/clif.rs` — бэкенд: `compile_object(&Program, &Checker) -> Vec<u8>`.
- `src/lib.rs` — `compile_clif_file(path)`: загрузка файла, проверка,
  затем объектный файл.
- `src/main.rs` — подкоманда `clif`: записать `.o`, собрать `cc` с
  `runtime/keel_clif_rt.c`, получить исполняемый файл.
- `runtime/keel_clif_rt.c` — печать значений (уже есть).
- `tests/clif.rs` — тесты на подмножество.
- `cranelift-backend.md` — пользовательская документация (появится вместе с бэкендом).

## Представление значений

- `int` → `I64`, `float` → `F64`, `bool` → `I8` (как результат `icmp`),
  `string` → указатель на данные модуля (`declare_data` + `global_value`).
- Указатели — тип указателя целевой ISA (`isa.pointer_type()`).
- Сравнения дают `I8`; `brif` принимает их напрямую (проверено на
  эксперименте: `icmp_imm_u` → `brif`).

## Тонкости, которые уже решены

- **Завершённые блоки.** После `jump`/`return_`/`brif` блок закрыт:
  следующую инструкцию в него добавлять нельзя. Вести свой флаг
  «блок завершён» и не добавлять `jump` в уже закрытый блок.
- **Блок-сводка после `if`.** Если обе ветви завершились (`return`),
  у сводки нет предшественников — нужен терминатор: `trap(UNREACHABLE)`.
- **Строковые литералы.** `declare_data` + `define_data` (с `\0` в конце),
  затем `declare_data_in_func` → `global_value`.
- **`main`.** Возвращаемое значение — `I32` (как у C `main`), `return;`
  в `main` → `0`.
