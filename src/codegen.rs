//! Генерация C из Keel.
//!
//! MVP-подмножество: int/bool/string, структуры, функции, ссылки (`&T` → `T*`).
//! Владение в рантайме не эмитится (это статическая гарантия Keel), но
//! ссылки компилируются в обычные указатели — вся безопасность доказана
//! статически.

use crate::ast::*;
use crate::checker::Checker;
use std::collections::HashMap;

pub struct Codegen<'a> {
    prog: &'a Program,
    types: &'a HashMap<u32, Type>,
    param_types: &'a HashMap<(String, String), Type>,
    /// id выражения `x.method(...)` → передать получателя ссылкой.
    method_recv_ref: &'a HashMap<u32, bool>,
    /// Конкретные инстанциации встроенных `Option<T>`.
    option_insts: &'a [Type],
    /// Конкретные инстанциации встроенных `Result<T, E>`.
    result_insts: &'a [(Type, Type)],
    /// Конкретные инстанциации встроенных `Vec<T>`.
    vec_insts: &'a [Type],
    /// Конкретные типы кортежей.
    tuple_insts: &'a [Type],
    /// Конкретные типы срезов.
    slice_insts: &'a [Type],
    /// `Vec`-локальные для автоматического освобождения в конце функции.
    vec_drops: &'a HashMap<String, Vec<(String, Type, Vec<String>)>>,
    /// Тела замыканий — эмитятся после функций.
    closures: std::cell::RefCell<Vec<String>>,
    /// Отложенные замыкания: (имя, параметры, тело, тип).
    pending_closures: std::cell::RefCell<Vec<(String, Vec<Param>, Block, Type)>>,
    out: std::cell::RefCell<String>,
    /// Имя C-типа для каждой структуры Keel.
    ctype: HashMap<String, String>,
    /// Объявления структур — нужны для конструкторов и составных литералов.
    structs: HashMap<String, StructDecl>,
    /// Объявления enum — нужны для вариантов и нулевого значения.
    enums: HashMap<String, EnumDecl>,
    /// Текущая глубина вложенности циклов — для уникальных имён
    /// переменных цикла.
    loop_depth: usize,
    /// Имена переменных цикла: имя в Keel -> имя в C. Нужно, потому что
    /// в C область  — всё тело, и вложенные циклы с одинаковым
    /// именем перетирают друг друга.
    loop_vars: HashMap<String, String>,
    /// Сейчас идёт инициализация структуры: литералы массивов в ней
    /// должны быть в форме `{...}`, а не составными литералами C.
    in_struct_init: std::cell::Cell<bool>,
    /// Текущее имя переменной в C для каждого имени в Keel. Второе
    /// `let x` затеняет первое, но в C это «redefinition» — поэтому
    /// затенённая переменная получает другое имя: `x`, `x__1`, `x__2`.
    ///
    /// Область видимости в Keel плоская, поэтому после `let x` все
    /// обращения к `x` относятся к новой переменной — и карта это знает.
    var_names: std::cell::RefCell<HashMap<String, String>>,
    /// Счётчик затенений по каждому имени.
    shadow_count: std::cell::RefCell<HashMap<String, usize>>,
    /// Генерируем ли прямо сейчас тело `main`. Нужно, чтобы голый
    /// `return;` в main превратился в `return 0;` (в C main возвращает
    /// int), а в обычной void-функции остался `return;`.
    in_main: bool,
    /// Тип возврата текущей функции. Нужен, чтобы дописать `return`
    /// в конце функции, возвращающей значение: если пользователь
    /// написал `fn f() int { if x { return 1; } }`, в C без финального
    /// `return` получается неопределённое поведение — gcc предупреждает
    /// «control reaches end of non-void function», но собирает.
    cur_ret: Type,
    /// Постусловия текущей функции: проверяются перед каждым `return`.
    cur_post: Vec<Expr>,
    /// Имя текущей функции для сообщения о нарушении контракта.
    cur_fn_name: String,
    /// Файл текущей функции — для `#line` в сгенерированном C.
    cur_file: String,
}

/// Вид определения типа для топологической сортировки.
enum DefKind {
    Struct(String),
    Enum(String),
    Option(Type),
    Result(Type, Type),
    Vec(Type),
    Tuple(Vec<Type>),
    Slice(Type),
}

impl<'a> Codegen<'a> {
    pub fn new(prog: &'a Program, checker: &'a Checker) -> Codegen<'a> {
        let mut ctype = HashMap::new();
        let mut structs = HashMap::new();
        let mut enums = HashMap::new();
        for item in &prog.items {
            match item {
                Item::Struct(s) => {
                    ctype.insert(s.name.clone(), format!("Keel_{}", s.name));
                    structs.insert(s.name.clone(), s.clone());
                }
                Item::Enum(e) => {
                    ctype.insert(e.name.clone(), format!("Keel_{}", e.name));
                    enums.insert(e.name.clone(), e.clone());
                }
                _ => {}
            }
        }
        Codegen {
            prog,
            types: &checker.expr_types,
            param_types: &checker.param_types,
            method_recv_ref: &checker.method_recv_ref,
            option_insts: &checker.option_insts,
            result_insts: &checker.result_insts,
            vec_insts: &checker.vec_insts,
            tuple_insts: &checker.tuple_insts,
            slice_insts: &checker.slice_insts,
            vec_drops: &checker.vec_drops,
            closures: std::cell::RefCell::new(Vec::new()),
            pending_closures: std::cell::RefCell::new(Vec::new()),
            out: std::cell::RefCell::new(String::new()),
            ctype,
            structs,
            enums,
            loop_depth: 0,
            loop_vars: HashMap::new(),
            in_struct_init: std::cell::Cell::new(false),
            shadow_count: std::cell::RefCell::new(HashMap::new()),
            var_names: std::cell::RefCell::new(HashMap::new()),
            in_main: false,
            cur_ret: Type::Void,
            cur_post: Vec::new(),
            cur_fn_name: String::new(),
            cur_file: String::new(),
        }
    }

    fn ty(&self, t: &Type) -> String {
        match t {
            Type::Int => "int64_t".to_string(),
            Type::Float => "double".to_string(),
            Type::Bool => "bool".to_string(),
            Type::Str => "const char*".to_string(),
            Type::Void => "void".to_string(),
            Type::Named(n) => self.ctype.get(n).cloned().unwrap_or_else(|| n.clone()),
            Type::Ref(inner) => match inner.as_ref() {
                // Указатель на массив: T (*)[N]
                Type::Array(elem, n) => format!("{}(*)[{}]", self.ty(elem), n),
                t => format!("{}*", self.ty(t)),
            },
            Type::Array(elem, n) => format!("{}[{}]", self.ty(elem), n),
            Type::Option(inner) => format!("Keel_Option_{}", inner.mangle()),
            Type::Result(ok, err) => {
                format!("Keel_Result_{}_{}", ok.mangle(), err.mangle())
            }
            Type::Vec(inner) => format!("Keel_Vec_{}", inner.mangle()),
            Type::Tuple(ts) => format!(
                "Keel_Tuple_{}",
                ts.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_")
            ),
            Type::Slice(inner) => format!("Keel_Slice_{}", inner.mangle()),
            Type::Fn(args, ret) => format!(
                "{} (*)({})",
                self.ty(ret),
                args.iter().map(|t| self.ty(t)).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// Тип элемента для составного литерала C: `(T[]){...}`.
    ///
    /// Здесь размеры пишутся ПОСЛЕ типа, а не после имени — это другое
    /// правило C, чем в декларации. Для вложенного массива `[int; 2]`
    /// тип элемента записывается как `int64_t[2]`, и литерал получается
    /// `(int64_t[2][]){...}`.
    fn array_literal_ty(&self, t: &Type) -> String {
        match t {
            Type::Array(elem, n) => format!("{}[{}]", self.array_literal_ty(elem), n),
            t => self.ty(t),
        }
    }

    /// Декларация переменной: `T name`, `T name[N]` или `T (*name)[N]`.
    ///
    /// В C размеры пишутся ПОСЛЕ имени, а не после типа: не `int64_t[2] x`,
    /// а `int64_t x[2]`. Для вложенных массивов это значит, что декларатор
    /// строится рекурсивно: `[[int; 2]; 2]` даёт `int64_t x[2][2]`,
    /// а не `int64_t[2] x[2]`.
    fn decl(&self, t: &Type, name: &str) -> String {
        match t {
            // Массив: размер дописывается к имени, рекурсия уходит в элемент.
            Type::Array(elem, n) => self.decl(elem, &format!("{}[{}]", name, n)),
            // Ссылка — это указатель: `T* name`.
            Type::Ref(inner) => match inner.as_ref() {
                // Указатель на массив: скобки обязательны — `T (*name)[N]`.
                Type::Array(elem, n) => {
                    self.decl(elem, &format!("(*{})[{}]", name, n))
                }
                t => format!("{}* {}", self.ty(t), name),
            },
            Type::Fn(args, ret) => format!(
                "{} (*{})({})",
                self.ty(ret),
                name,
                args.iter().map(|t| self.ty(t)).collect::<Vec<_>>().join(", ")
            ),
            t => format!("{} {}", self.ty(t), name),
        }
    }

    /// Декларация параметра: `&T` получает `restrict` — модель владения
    /// гарантирует, что изменяемые пути не пересекаются.
    fn param_decl(&self, t: &Type, name: &str) -> String {
        match t {
            Type::Ref(inner) => match inner.as_ref() {
                Type::Array(elem, n) => self.decl(elem, &format!("(*{})[{}]", name, n)),
                t => format!("{}* restrict {}", self.ty(t), name),
            },
            t => self.decl(t, name),
        }
    }

    pub fn generate(mut self) -> String {
        self.out.borrow_mut().push_str("#include <stdint.h>\n#include <stdbool.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n");
        // Нарушение контракта pre/post: сообщение в stderr и выход.
        // Это проверка, а не доказательство: контракт говорит «здесь
        // значение гарантированно есть», а не «поверьте».
        self.out.borrow_mut().push_str(
            "// Нарушение контракта pre/post: сообщение и выход с кодом 1.\n\
             static void k_contract_fail(const char* fn, const char* kind, int line) {\n\
             \x20 fprintf(stderr, \"нарушен контракт %s функции `%s` (строка %d)\\n\", kind, fn, line);\n\
             \x20 exit(1);\n\
             }\n\n",
        );
        // Проверка границ индексации: выход за границы — ошибка, а не
        // чтение чужой памяти.
        self.out.borrow_mut().push_str(
            "// Проверка границ индексации.\n\
             static int64_t k_index_check(int64_t i, int64_t len, int line) {\n\
             \x20 if (i < 0 || i >= len) {\n\
             \x20   fprintf(stderr, \"строка %d: индекс %lld вне границ [0, %lld)\\n\", line, (long long)i, (long long)len);\n\
             \x20   exit(1);\n\
             \x20 }\n\
             \x20 return i;\n\
             }\n\n",
        );
        // Ввод: строка и целое из stdin.
        //
        // `read_line` читает строку целиком (до '\n'), а не по слову, как
        // `scanf("%s")`: в имени может быть пробел. Буфер растёт по мере
        // надобности; память не освобождается — см. волну 8.
        //
        // `read_int` читает СТРОКУ и разбирает её через `strtoll`, а не
        // `scanf("%lld")`. Это принципиально: `scanf` оставляет '\n' в
        // буфере ввода, и следующий `read_line` вернул бы пустую строку.
        // Диагностика на нечисловой ввод — в самой программе (`read_int`
        // возвращает 0, как `atoi` в C).
        self.out.borrow_mut().push_str(
            "// Читает строку из stdin до '\\n' (сам '\\n' не входит).\n\
             // Возвращает NULL в конце ввода (EOF).\n\
             // Память не освобождается — см. волну 8 в docs/lang-design.md.\n\
             static char* k_read_line(void) {\n\
             \x20 size_t cap = 64, len = 0;\n\
             \x20 char* buf = (char*)malloc(cap);\n\
             \x20 if (!buf) return NULL;\n\
             \x20 int c;\n\
             \x20 while ((c = getchar()) != EOF && c != '\\n') {\n\
             \x20   if (len + 1 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }\n\
             \x20   buf[len++] = (char)c;\n\
             \x20 }\n\
             \x20 if (c == EOF && len == 0) { free(buf); return NULL; }\n\
             \x20 buf[len] = '\\0';\n\
             \x20 return buf;\n\
             }\n\n\
             // Читает целое из stdin. Читает СТРОКУ и разбирает её —\n\
             // иначе перевод строки остался бы в буфере ввода\n\
             // и сломал бы следующий read_line.\n\
             static int64_t k_read_int(void) {\n\
             \x20 char* s = k_read_line();\n\
             \x20 if (!s) return 0;\n\
             \x20 return (int64_t)strtoll(s, NULL, 10);\n\
             }\n\n\
             // То же, но NULL заменяется пустой строкой: в языке нет\n\
             // Option, а печатать NULL нельзя.\n\
             static const char* k_read_line_or_empty(void) {\n\
             \x20 char* s = k_read_line();\n\
             \x20 return s ? s : \"\";\n\
             }\n\n\
             // Аргументы командной строки. main() кладёт их сюда, чтобы\n\
             // argc()/argv(i) работали в ЛЮБОЙ функции, а не только в main.\n\
             static int64_t k_argc = 0;\n\
             static char** k_argv = 0;\n\n",
        );
        // Файловый ввод/вывод. fd — это FILE*, упакованный в int64_t.
        // -1 — ошибка открытия.
        self.out.borrow_mut().push_str(
            "static int64_t k_open(const char* path, const char* mode) {\n\
             \x20 FILE* f = fopen(path, mode);\n\
             \x20 return f ? (int64_t)(intptr_t)f : -1;\n\
             }\n\
             static char* k_read(int64_t fd) {\n\
             \x20 FILE* f = (FILE*)(intptr_t)fd;\n\
             \x20 if (!f) return \"\";\n\
             \x20 size_t cap = 256, len = 0;\n\
             \x20 char* buf = (char*)malloc(cap);\n\
             \x20 if (!buf) return \"\";\n\
             \x20 for (;;) {\n\
             \x20   if (len + 1 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }\n\
             \x20   size_t n = fread(buf + len, 1, cap - len - 1, f);\n\
             \x20   len += n;\n\
             \x20   if (n == 0) break;\n\
             \x20 }\n\
             \x20 buf[len] = '\\0';\n\
             \x20 return buf;\n\
             }\n\
             static void k_write(int64_t fd, const char* s) {\n\
             \x20 FILE* f = (FILE*)(intptr_t)fd;\n\
             \x20 if (f) fputs(s, f);\n\
             }\n\
             static void k_close(int64_t fd) {\n\
             \x20 FILE* f = (FILE*)(intptr_t)fd;\n\
             \x20 if (f) fclose(f);\n\
             }\n\n",
        );
        // Конкатенация строк: результат требует памяти, которой у операндов
        // нет. Копируем в свежий буфер из кучи.
        //
        // Память не освобождается: в Keel пока нет ни `free`, ни владения
        // для кучи. Для коротких программ это незаметно; настоящее
        // освобождение — в волне 8 (куча и растущие коллекции).
        self.out.borrow_mut().push_str(
            "// Склеивает две строки в новый буфер из кучи.\n\
             // Освобождения пока нет — см. волну 8 в docs/lang-design.md.\n\
             static char* k_str_concat(const char* a, const char* b) {\n\
             \x20 size_t la = strlen(a), lb = strlen(b);\n\
             \x20 char* r = (char*)malloc(la + lb + 1);\n\
             \x20 memcpy(r, a, la);\n\
             \x20 memcpy(r + la, b, lb + 1);\n\
             \x20 return r;\n\
             }\n\n",
        );

        // Простые enum'ы (без данных) — forward-декларация enum в C
        // невозможна, поэтому объявляем их сразу.
        for item in &self.prog.items {
            if let Item::Enum(e) = item {
                if e.variants.iter().all(|v| v.payload.is_empty()) {
                    self.emit_enum(e);
                }
            }
        }
        // Forward-декларации всех struct-подобных типов: позволяет
        // определять их в порядке зависимостей.
        self.emit_forward_decls();
        self.emit_type_defs();
        self.out.borrow_mut().push('\n');
        // Конструкторы структур с массивами в полях.
        for item in &self.prog.items {
            if let Item::Struct(s) = item {
                self.emit_struct_ctor(s);
            }
        }
        // Прототипы: в C функция должна быть объявлена до вызова.
        // Все функции: свободные и из impl-блоков.
        let mut all_funcs: Vec<&FuncDecl> = Vec::new();
        for item in &self.prog.items {
            match item {
                Item::Func(f) => all_funcs.push(f),
                Item::Impl(im) => all_funcs.extend(im.funcs.iter()),
                Item::Struct(_) => {}
                Item::Enum(_) => {}
                // Импорты обрабатывает загрузчик модулей: до
                // проверщика и генератора они не доходят.
                Item::Import(_) => {}
            }
        }
        for f in &all_funcs {
            {
                if f.name == "main" {
                    continue;
                }
                let mut params = Vec::new();
                for p in &f.params {
                    let pt = match &p.kind {
                        ParamKind::Owned => p.ty.clone(),
                        ParamKind::Ref => Type::Ref(Box::new(p.ty.clone())),
                        ParamKind::RefIn(_) => {
                            let inner = self
                                .param_types
                                .get(&(crate::checker::Checker::func_key(f), p.name.clone()))
                                .cloned()
                                .unwrap_or_else(|| p.ty.clone());
                            Type::Ref(Box::new(inner))
                        }
                    };
                    params.push(self.param_decl(&pt, &p.name));
                }
                let decl = self.func_decl(f, &params);
                self.out.borrow_mut().push_str(&format!("static {};\n", decl));
            }
        }
        self.out.borrow_mut().push('\n');

        // Собираем замыкания заранее: их тела эмитятся до функций,
        // чтобы main мог на них ссылаться.
        for f in &all_funcs {
            for stmt in &f.body.stmts {
                self.collect_closures_stmt(stmt);
            }
        }
        // Генерируем тела замыканий (вложенные добавляются по ходу).
        loop {
            let pending = std::mem::take(&mut *self.pending_closures.borrow_mut());
            if pending.is_empty() {
                break;
            }
            for (name, params, body, ret) in pending {
                let ret_ty = match &ret {
                    Type::Fn(_, r) => self.ty(r),
                    _ => "void".to_string(),
                };
                let args: Vec<String> =
                    params.iter().map(|p| self.decl(&p.ty, &p.name)).collect();
                let mut s = format!("static {} {}({}) {{\n", ret_ty, name, args.join(", "));
                let saved = std::mem::take(&mut *self.out.borrow_mut());
                for (i, stmt) in body.stmts.iter().enumerate() {
                    if i == body.stmts.len() - 1 {
                        if let Stmt::Expr(e) = stmt {
                            let v = self.expr(e);
                            self.out.borrow_mut().push_str(&format!("  return {};\n", v));
                            continue;
                        }
                    }
                    self.emit_stmt(stmt);
                }
                let body_out = std::mem::replace(&mut *self.out.borrow_mut(), saved);
                s.push_str(&body_out);
                s.push_str("}\n\n");
                self.closures.borrow_mut().push(s);
            }
        }
        // Эмитим тела замыканий до функций.
        let closures = std::mem::take(&mut *self.closures.borrow_mut());
        for c in &closures {
            self.out.borrow_mut().push_str(c);
        }

        for item in &self.prog.items {
            match item {
                Item::Func(f) => self.emit_func(f),
                Item::Impl(im) => {
                    for f in &im.funcs {
                        self.emit_func(f);
                    }
                }
                Item::Struct(_) => {}
                Item::Enum(_) => {}
                // Импорты обрабатывает загрузчик модулей: до
                // проверщика и генератора они не доходят.
                Item::Import(_) => {}
            }
        }
        self.out.into_inner()
    }

    /// Встроенные `Option<T>`/`Result<T,E>`: мономорфизация в C.
    /// Все определения типов в порядке зависимостей.
    ///
    /// `Option<Vec<int>>` требует `Keel_Vec_int`, `Vec<Option<int>>` —
    /// `Keel_Option_int`, а структура с полем `Vec<int>` — `Keel_Vec_int`.
    /// Поэтому — топологическая сортировка всех определений сразу.
    fn emit_type_defs(&mut self) {
        let mut defs: Vec<(String, DefKind, Vec<Type>)> = Vec::new();
        for (name, s) in &self.structs {
            let deps: Vec<Type> = s.fields.iter().map(|f| f.ty.clone()).collect();
            defs.push((format!("Keel_{}", name), DefKind::Struct(name.clone()), deps));
        }
        for (name, e) in &self.enums {
            if e.variants.iter().any(|v| !v.payload.is_empty()) {
                let deps: Vec<Type> = e.variants.iter().flat_map(|v| v.payload.clone()).collect();
                defs.push((format!("Keel_{}", name), DefKind::Enum(name.clone()), deps));
            }
        }
        for t in self.option_insts {
            let Type::Option(inner) = t else { continue };
            defs.push((
                format!("Keel_Option_{}", inner.mangle()),
                DefKind::Option((**inner).clone()),
                vec![(**inner).clone()],
            ));
        }
        for (ok, err) in self.result_insts {
            defs.push((
                format!("Keel_Result_{}_{}", ok.mangle(), err.mangle()),
                DefKind::Result(ok.clone(), err.clone()),
                vec![ok.clone(), err.clone()],
            ));
        }
        for t in self.vec_insts {
            let Type::Vec(inner) = t else { continue };
            defs.push((
                format!("Keel_Vec_{}", inner.mangle()),
                DefKind::Vec((**inner).clone()),
                vec![(**inner).clone()],
            ));
        }
        for t in self.tuple_insts {
            let Type::Tuple(ts) = t else { continue };
            defs.push((
                format!(
                    "Keel_Tuple_{}",
                    ts.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_")
                ),
                DefKind::Tuple(ts.clone()),
                ts.clone(),
            ));
        }
        for t in self.slice_insts {
            let Type::Slice(inner) = t else { continue };
            defs.push((
                format!("Keel_Slice_{}", inner.mangle()),
                DefKind::Slice((**inner).clone()),
                vec![(**inner).clone()],
            ));
        }
        let mut ordered: Vec<String> = Vec::new();
        let mut visiting: Vec<String> = Vec::new();
        for (name, _, _) in &defs {
            self.visit_def(name, &defs, &mut ordered, &mut visiting);
        }
        for name in &ordered {
            if let Some((_, kind, _)) = defs.iter().find(|(n, _, _)| n == name) {
                match kind {
                    DefKind::Struct(sn) => {
                        if let Some(s) = self.structs.get(sn).cloned() {
                            self.emit_struct(&s);
                        }
                    }
                    DefKind::Enum(en) => {
                        if let Some(e) = self.enums.get(en).cloned() {
                            self.emit_enum(&e);
                        }
                    }
                    DefKind::Option(inner) => self.emit_option_def(inner),
                    DefKind::Result(ok, err) => self.emit_result_def(ok, err),
                    DefKind::Vec(inner) => self.emit_vec_def(inner),
                    DefKind::Tuple(ts) => self.emit_tuple_def(ts),
                    DefKind::Slice(inner) => self.emit_slice_def(inner),
                }
            }
        }
        // Функции Vec — после всех определений: они возвращают Option<T>
        // и принимают элементы по значению (нужен полный тип).
        for t in self.vec_insts {
            let Type::Vec(inner) = t else { continue };
            self.emit_vec_funcs(inner);
        }
    }

    fn visit_def(
        &self,
        name: &str,
        defs: &[(String, DefKind, Vec<Type>)],
        ordered: &mut Vec<String>,
        visiting: &mut Vec<String>,
    ) {
        if ordered.iter().any(|n| n == name) || visiting.iter().any(|n| n == name) {
            return;
        }
        visiting.push(name.to_string());
        if let Some((_, _, deps)) = defs.iter().find(|(n, _, _)| n == name) {
            for dep in deps {
                if let Some(dn) = self.type_def_name(dep) {
                    self.visit_def(&dn, defs, ordered, visiting);
                }
            }
        }
        visiting.pop();
        ordered.push(name.to_string());
    }

    fn type_def_name(&self, t: &Type) -> Option<String> {
        match t {
            Type::Named(n) => {
                if self.structs.contains_key(n) || self.enums.contains_key(n) {
                    Some(format!("Keel_{}", n))
                } else {
                    None
                }
            }
            Type::Option(inner) => Some(format!("Keel_Option_{}", inner.mangle())),
            Type::Result(ok, err) => Some(format!("Keel_Result_{}_{}", ok.mangle(), err.mangle())),
            Type::Vec(inner) => Some(format!("Keel_Vec_{}", inner.mangle())),
            Type::Tuple(ts) => Some(format!(
                "Keel_Tuple_{}",
                ts.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_")
            )),
            Type::Slice(inner) => Some(format!("Keel_Slice_{}", inner.mangle())),
            Type::Ref(inner) => self.type_def_name(inner),
            Type::Array(inner, _) => self.type_def_name(inner),
            _ => None,
        }
    }

    fn emit_option_def(&mut self, inner: &Type) {
        let cname = format!("Keel_Option_{}", inner.mangle());
        let field = self.decl(inner, "_0");
        self.out.borrow_mut().push_str(&format!(
            "typedef enum {{\n  {}_None = 0,\n  {}_Some = 1\n}} {}_tag;\n\n",
            cname, cname, cname
        ));
        self.out.borrow_mut().push_str(&format!(
            "struct {} {{\n  {}_tag tag;\n  union {{\n    struct {{\n      {};\n    }} Some;\n  }} data;\n}};\n\n",
            cname, cname, field
        ));
    }

    fn emit_result_def(&mut self, ok: &Type, err: &Type) {
        let cname = format!("Keel_Result_{}_{}", ok.mangle(), err.mangle());
        let ok_field = self.decl(ok, "_0");
        let err_field = self.decl(err, "_0");
        self.out.borrow_mut().push_str(&format!(
            "typedef enum {{\n  {}_Ok = 0,\n  {}_Err = 1\n}} {}_tag;\n\n",
            cname, cname, cname
        ));
        self.out.borrow_mut().push_str(&format!(
            "struct {} {{\n  {}_tag tag;\n  union {{\n    struct {{\n      {};\n    }} Ok;\n    struct {{\n      {};\n    }} Err;\n  }} data;\n}};\n\n",
            cname, cname, ok_field, err_field
        ));
    }

    fn emit_vec_def(&mut self, inner: &Type) {
        let cname = format!("Keel_Vec_{}", inner.mangle());
        let elem = self.ty(inner);
        self.out.borrow_mut().push_str(&format!(
            "struct {} {{\n  {}* data;\n  int64_t len;\n  int64_t cap;\n}};\n\n",
            cname, elem
        ));
    }

    fn emit_tuple_def(&mut self, ts: &[Type]) {
        let cname = format!(
            "Keel_Tuple_{}",
            ts.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_")
        );
        self.out.borrow_mut().push_str("typedef struct {\n");
        for (i, t) in ts.iter().enumerate() {
            let d = self.decl(t, &format!("_{}", i));
            self.out.borrow_mut().push_str(&format!("  {};\n", d));
        }
        self.out.borrow_mut().push_str(&format!("}} {};\n\n", cname));
    }

    fn emit_slice_def(&mut self, inner: &Type) {
        let cname = format!("Keel_Slice_{}", inner.mangle());
        let elem = self.ty(inner);
        self.out.borrow_mut().push_str(&format!(
            "typedef struct {{\n  {}* data;\n  int64_t len;\n}} {};\n\n",
            elem, cname
        ));
    }

    fn emit_vec_funcs(&mut self, inner: &Type) {
        let m = inner.mangle();
        let cname = format!("Keel_Vec_{}", m);
        let elem = self.ty(inner);
        let opt = format!("Keel_Option_{}", m);
        self.out.borrow_mut().push_str(&format!(
            "static {cname} k_vec_{m}_new(void) {{\n  {cname} v = {{ NULL, 0, 0 }};\n  return v;\n}}\n\n",
            cname = cname, m = m
        ));
        self.out.borrow_mut().push_str(&format!(
            "static void k_vec_{m}_push({cname}* v, {elem} x) {{\n  if (v->len == v->cap) {{\n    v->cap = v->cap ? v->cap * 2 : 4;\n    v->data = ({elem}*)realloc(v->data, (size_t)v->cap * sizeof({elem}));\n  }}\n  v->data[v->len++] = x;\n}}\n\n",
            m = m, cname = cname, elem = elem
        ));
        self.out.borrow_mut().push_str(&format!(
            "static int64_t k_vec_{m}_len(const {cname}* v) {{ return v->len; }}\n\n",
            m = m, cname = cname
        ));
        self.out.borrow_mut().push_str(&format!(
            "static {opt} k_vec_{m}_get(const {cname}* v, int64_t i) {{\n  if (i < 0 || i >= v->len) return ({opt}){{ .tag = {opt}_None }};\n  return ({opt}){{ .tag = {opt}_Some, .data.Some = {{ ._0 = v->data[i] }} }};\n}}\n\n",
            opt = opt, m = m, cname = cname
        ));
        self.out.borrow_mut().push_str(&format!(
            "static {opt} k_vec_{m}_pop({cname}* v) {{\n  if (v->len == 0) return ({opt}){{ .tag = {opt}_None }};\n  return ({opt}){{ .tag = {opt}_Some, .data.Some = {{ ._0 = v->data[--v->len] }} }};\n}}\n\n",
            opt = opt, m = m, cname = cname
        ));
        self.out.borrow_mut().push_str(&format!(
            "static void k_vec_{m}_free({cname}* v) {{\n  free(v->data);\n  v->data = NULL;\n  v->len = 0;\n  v->cap = 0;\n}}\n\n",
            m = m, cname = cname
        ));
    }

    fn emit_forward_decls(&mut self) {
        for item in &self.prog.items {
            match item {
                Item::Struct(s) => {
                    let cname = self.ctype.get(&s.name).cloned().unwrap_or_default();
                    self.out.borrow_mut().push_str(&format!("typedef struct {} {};\n", cname, cname));
                }
                Item::Enum(e) => {
                    if e.variants.iter().any(|v| !v.payload.is_empty()) {
                        let cname = self.ctype.get(&e.name).cloned().unwrap_or_default();
                        self.out.borrow_mut().push_str(&format!("typedef struct {} {};\n", cname, cname));
                    }
                }
                _ => {}
            }
        }
        for t in self.option_insts {
            let Type::Option(inner) = t else { continue };
            let cname = format!("Keel_Option_{}", inner.mangle());
            self.out.borrow_mut().push_str(&format!("typedef struct {} {};\n", cname, cname));
        }
        for (ok, err) in self.result_insts {
            let cname = format!("Keel_Result_{}_{}", ok.mangle(), err.mangle());
            self.out.borrow_mut().push_str(&format!("typedef struct {} {};\n", cname, cname));
        }
        for t in self.vec_insts {
            let Type::Vec(inner) = t else { continue };
            let cname = format!("Keel_Vec_{}", inner.mangle());
            self.out.borrow_mut().push_str(&format!("typedef struct {} {};\n", cname, cname));
        }
        self.out.borrow_mut().push('\n');
    }

    fn emit_struct(&mut self, s: &StructDecl) {
        let cname = self.ctype.get(&s.name).cloned().unwrap_or_default();
        self.out.borrow_mut().push_str(&format!("struct {} {{\n", cname));
        for f in &s.fields {
            self.out.borrow_mut().push_str(&format!("  {};\n", self.decl(&f.ty, &f.name)));
        }
        self.out.borrow_mut().push_str("};\n\n");
    }

    /// `enum` → C. Без данных — обычный C enum; с данными — тег +
    /// union, чтобы вариант нёс значение. Константы получают префикс
    /// `Keel_Тип_`, чтобы варианты разных enum не сталкивались.
    fn emit_enum(&mut self, e: &EnumDecl) {
        let cname = self.ctype.get(&e.name).cloned().unwrap_or_default();
        let has_payload = e.variants.iter().any(|v| !v.payload.is_empty());
        let mut tags = Vec::new();
        for (i, v) in e.variants.iter().enumerate() {
            tags.push(format!("  Keel_{}_{} = {}", e.name, v.name, i));
        }
        if !has_payload {
            self.out.borrow_mut().push_str(&format!(
                "typedef enum {{\n{}\n}} {};\n\n",
                tags.join(",\n"),
                cname
            ));
            return;
        }
        self.out.borrow_mut().push_str(&format!(
            "typedef enum {{\n{}\n}} {}_tag;\n\n",
            tags.join(",\n"),
            cname
        ));
        let mut members = Vec::new();
        for v in &e.variants {
            if v.payload.is_empty() {
                continue;
            }
            let mut fields = Vec::new();
            for (i, t) in v.payload.iter().enumerate() {
                fields.push(format!("    {};", self.decl(t, &format!("_{}", i))));
            }
            members.push(format!(
                "    struct {{\n{}\n    }} {};",
                fields.join("\n"),
                v.name
            ));
        }
        self.out.borrow_mut().push_str(&format!(
            "struct {} {{\n  {}_tag tag;\n  union {{\n{}\n  }} data;\n}};\n\n",
            cname,
            cname,
            members.join("\n")
        ));
    }

    /// Конструктор структуры, у которой есть поле-массив.
    ///
    /// В C нельзя присвоить массив в поле через инициализатор
    /// (`.f = arr` — ошибка), поэтому делаем отдельную функцию, которая
    /// копирует поле через memcpy.
    fn emit_struct_ctor(&mut self, s: &StructDecl) {
        if !s.fields.iter().any(|f| matches!(f.ty, Type::Array(_, _))) {
            return;
        }
        let cname = self.ctype.get(&s.name).cloned().unwrap_or_default();
        let mut params = Vec::new();
        let mut body = Vec::new();
        for f in &s.fields {
            match &f.ty {
                // Массив приходит указателем на элемент.
                Type::Array(elem, _) => {
                    params.push(format!("{}* {}", self.ty(elem), f.name));
                    body.push(format!(
                        "  memcpy(r.{}, {}, sizeof(r.{}));",
                        f.name, f.name, f.name
                    ));
                }
                t => {
                    params.push(self.decl(t, &f.name));
                    body.push(format!("  r.{} = {};", f.name, f.name));
                }
            }
        }
        let mut text = format!(
            "static {} k_new_{}({}) {{\n  {} r;\n",
            cname,
            s.name,
            params.join(", "),
            cname
        );
        for line in &body {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("  return r;\n}\n\n");
        self.out.borrow_mut().push_str(&text);
    }

    fn collect_closures_expr(&self, e: &Expr) {
        match &e.kind {
            ExprKind::Closure { params, body } => {
                let name = format!(
                    "closure_{}",
                    self.closures.borrow().len() + self.pending_closures.borrow().len()
                );
                let ret = self.types.get(&e.id).cloned().unwrap_or(Type::Void);
                self.pending_closures
                    .borrow_mut()
                    .push((name, params.clone(), body.clone(), ret));
                for stmt in &body.stmts {
                    self.collect_closures_stmt(stmt);
                }
            }
            ExprKind::Field { obj, .. } => self.collect_closures_expr(obj),
            ExprKind::Index { obj, idx } => {
                self.collect_closures_expr(obj);
                self.collect_closures_expr(idx);
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for i in items {
                    self.collect_closures_expr(i);
                }
            }
            ExprKind::Call { callee, args } => {
                self.collect_closures_expr(callee);
                for a in args {
                    self.collect_closures_expr(a);
                }
            }
            ExprKind::MethodCall { obj, args, .. } => {
                self.collect_closures_expr(obj);
                for a in args {
                    self.collect_closures_expr(a);
                }
            }
            ExprKind::Unary { e, .. } => self.collect_closures_expr(e),
            ExprKind::Binary { l, r, .. } => {
                self.collect_closures_expr(l);
                self.collect_closures_expr(r);
            }
            ExprKind::Ref(inner) => self.collect_closures_expr(inner),
            ExprKind::If { cond, then_expr, else_expr } => {
                self.collect_closures_expr(cond);
                self.collect_closures_expr(then_expr);
                self.collect_closures_expr(else_expr);
            }
            ExprKind::StructInit { fields, .. } => {
                for (_, e) in fields {
                    self.collect_closures_expr(e);
                }
            }
            _ => {}
        }
    }

    fn collect_closures_stmt(&self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { init, .. } => self.collect_closures_expr(init),
            Stmt::Assign { lhs, rhs } => {
                self.collect_closures_expr(lhs);
                self.collect_closures_expr(rhs);
            }
            Stmt::Expr(e) => self.collect_closures_expr(e),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.collect_closures_expr(e);
                }
            }
            Stmt::If { cond, then_block, else_block } => {
                self.collect_closures_expr(cond);
                for s in &then_block.stmts {
                    self.collect_closures_stmt(s);
                }
                if let Some(b) = else_block {
                    for s in &b.stmts {
                        self.collect_closures_stmt(s);
                    }
                }
            }
            Stmt::While { cond, body } => {
                self.collect_closures_expr(cond);
                for s in &body.stmts {
                    self.collect_closures_stmt(s);
                }
            }
            Stmt::DoWhile { body, cond } => {
                for s in &body.stmts {
                    self.collect_closures_stmt(s);
                }
                self.collect_closures_expr(cond);
            }
            Stmt::ForRange { start, end, body, .. } => {
                self.collect_closures_expr(start);
                self.collect_closures_expr(end);
                for s in &body.stmts {
                    self.collect_closures_stmt(s);
                }
            }
            Stmt::ForEach { iter, body, .. } => {
                self.collect_closures_expr(iter);
                for s in &body.stmts {
                    self.collect_closures_stmt(s);
                }
            }
            Stmt::Match { subject, arms, .. } => {
                self.collect_closures_expr(subject);
                for arm in arms {
                    for s in &arm.body.stmts {
                        self.collect_closures_stmt(s);
                    }
                }
            }
            _ => {}
        }
    }

    /// Декларация функции: `Ret name(params)`, а для возврата
    /// функции-указателя — `Ret (*name(params))(args)`.
    fn func_decl(&self, f: &FuncDecl, params: &[String]) -> String {
        match &f.ret {
            Type::Fn(args, ret) => format!(
                "{} (*{}({}))({})",
                self.ty(ret),
                self.fname_of(f),
                params.join(", "),
                args.iter().map(|t| self.ty(t)).collect::<Vec<_>>().join(", ")
            ),
            t => format!("{} {}({})", self.ty(t), self.fname_of(f), params.join(", ")),
        }
    }

    fn emit_func(&mut self, f: &FuncDecl) {
        // Имена переменных — свои в каждой функции.
        self.var_names.borrow_mut().clear();
        self.shadow_count.borrow_mut().clear();
        self.in_main = f.name == "main";
        self.cur_ret = f.ret.clone();
        self.cur_post = f.post.clone();
        self.cur_fn_name = match &f.owner {
            Some(ty) => format!("{}::{}", ty, f.name),
            None => f.name.clone(),
        };
        self.cur_file = if f.file.is_empty() { "<keel>".to_string() } else { f.file.clone() };
        let mut params = Vec::new();
        for p in &f.params {
            let pt = match &p.kind {
                ParamKind::Owned => p.ty.clone(),
                ParamKind::Ref => Type::Ref(Box::new(p.ty.clone())),
                // Тип параметра-внутри выведен проверщиком.
                ParamKind::RefIn(_) => {
                    let inner = self
                        .param_types
                        .get(&(crate::checker::Checker::func_key(f), p.name.clone()))
                        .cloned()
                        .unwrap_or_else(|| p.ty.clone());
                    Type::Ref(Box::new(inner))
                }
            };
            params.push(self.param_decl(&pt, &p.name));
            // Параметр занимает имя в C — регистрируем, чтобы `let` с тем же
            // именем не переопределял его.
            self.shadow_count
                .borrow_mut()
                .insert(p.name.clone(), 1);
            self.var_names
                .borrow_mut()
                .insert(p.name.clone(), p.name.clone());
        }
        if f.name == "main" {
            // `argc`/`argv` доступны из программы: main принимает их
            // как в C и сразу кладёт в глобальные k_argc/k_argv, чтобы
            // встроенные argc()/argv(i) работали в любой функции.
            self.out.borrow_mut().push_str("int main(int __k_argc, char** __k_argv) {\n");
            self.out.borrow_mut().push_str("  k_argc = __k_argc;\n");
            self.out.borrow_mut().push_str("  k_argv = __k_argv;\n");
        } else {
            let decl = self.func_decl(f, &params);
            self.out.borrow_mut().push_str(&format!("static {} {{\n", decl));
        }

        // Предусловия — на входе в функцию, после связывания параметров.
        for e in &f.pre {
            let cond = self.expr(e);
            self.out.borrow_mut().push_str(&format!(
                "  if (!({})) {{ k_contract_fail(\"{}\", \"pre\", {}); }}\n",
                cond, self.cur_fn_name, e.line
            ));
        }

        // Liveness: для каждого Vec-локального находим последний
        // top-level оператор, который его упоминает, и освобождаем
        // сразу после него. Не упомянутые вовсе — в конце.
        let drops = self
            .vec_drops
            .get(&crate::checker::Checker::func_key(f))
            .cloned()
            .unwrap_or_default();
        let last_use: Vec<(String, Type, usize)> = drops
            .into_iter()
            .map(|(name, ty, aliases)| {
                let idx = f
                    .body
                    .stmts
                    .iter()
                    .rposition(|s| {
                        stmt_mentions(s, &name)
                            || aliases.iter().any(|a| stmt_mentions(s, a))
                    })
                    .unwrap_or(usize::MAX);
                (name, ty, idx)
            })
            .collect();
        for (i, stmt) in f.body.stmts.iter().enumerate() {
            self.emit_stmt(stmt);
            for (name, ty, idx) in &last_use {
                if *idx == i {
                    if let Type::Vec(inner) = ty {
                        let m = inner.mangle();
                        let cname = self.var_name(name);
                        self.out.borrow_mut().push_str(&format!("  k_vec_{}_free(&{});\n", m, cname));
                    }
                }
            }
        }
        for (name, ty, idx) in &last_use {
            if *idx == usize::MAX {
                if let Type::Vec(inner) = ty {
                    let m = inner.mangle();
                    let cname = self.var_name(name);
                    self.out.borrow_mut().push_str(&format!("  k_vec_{}_free(&{});\n", m, cname));
                }
            }
        }
        // Неявный возврат в конце функции.
        if f.name == "main" {
            self.emit_implicit_return("0", &Type::Int);
        } else if f.ret != Type::Void {
            // Функция возвращает значение, но тело может закончиться без
            // `return` (например, `fn f(x int) int { if x > 0 { return 1; } }`).
            // В C это неопределённое поведение: gcc предупреждает
            // «control reaches end of non-void function», но собирает, и
            // функция вернёт мусор. Добавляем возврат нуля, чтобы
            // поведение было определено.
            let zero = self.zero_value(&f.ret);
            let ret = f.ret.clone();
            self.emit_implicit_return(&zero, &ret);
        } else if !self.cur_post.is_empty() {
            self.emit_post_checks();
        }
        self.out.borrow_mut().push_str("}\n\n");
    }

    /// Возврат в конце функции, если тело не закончилось `return`.
    fn emit_implicit_return(&mut self, value: &str, ty: &Type) {
        if self.cur_post.is_empty() {
            self.out.borrow_mut().push_str(&format!("  return {};\n", value));
            return;
        }
        self.out.borrow_mut().push_str("  {\n");
        self.out.borrow_mut().push_str(&format!(
            "    {} = {};\n",
            self.decl(ty, "__k_result"),
            value
        ));
        self.emit_post_checks();
        self.out.borrow_mut().push_str("    return __k_result;\n");
        self.out.borrow_mut().push_str("  }\n");
    }

    /// Проверки постусловий. Вызывается после того, как возвращаемое
    /// значение положено в `__k_result`: имя `result` в выражениях
    /// указывает на него.
    fn emit_post_checks(&mut self) {
        if self.cur_post.is_empty() {
            return;
        }
        let post = self.cur_post.clone();
        let saved = self
            .var_names
            .borrow_mut()
            .insert("result".to_string(), "__k_result".to_string());
        for e in &post {
            let cond = self.expr(e);
            self.out.borrow_mut().push_str(&format!(
                "    if (!({})) {{ k_contract_fail(\"{}\", \"post\", {}); }}\n",
                cond, self.cur_fn_name, e.line
            ));
        }
        match saved {
            Some(v) => {
                self.var_names.borrow_mut().insert("result".to_string(), v);
            }
            None => {
                self.var_names.borrow_mut().remove("result");
            }
        }
    }

    /// Нулевое значение типа — для возврата в конце функции, которая
    /// может закончиться без `return`.
    fn zero_value(&self, t: &Type) -> String {
        match t {
            Type::Int => "INT64_C(0)".to_string(),
            Type::Float => "0.0".to_string(),
            Type::Bool => "false".to_string(),
            Type::Str => "\"\"".to_string(),
            Type::Ref(_) => "NULL".to_string(),
            Type::Named(n) => {
                if let Some(e) = self.enums.get(n) {
                    let first = &e.variants[0];
                    if e.variants.iter().any(|v| !v.payload.is_empty()) {
                        let c = self.ctype.get(n).cloned().unwrap_or_else(|| n.clone());
                        return format!("({}){{ .tag = Keel_{}_{} }}", c, n, first.name);
                    }
                    // Нулевое значение enum без данных — первый вариант:
                    // у C enum нет «нуля» как отдельного значения.
                    return format!("Keel_{}_{}", n, first.name);
                }
                let c = self.ctype.get(n).cloned().unwrap_or_else(|| n.clone());
                format!("({}){{0}}", c)
            }
            // Массив вернуть из функции в C нельзя — сюда не дойдём.
            Type::Option(inner) => {
                let c = format!("Keel_Option_{}", inner.mangle());
                format!("({}){{ .tag = {}_None }}", c, c)
            }
            Type::Result(ok, err) => {
                let c = format!("Keel_Result_{}_{}", ok.mangle(), err.mangle());
                format!("({}){{ .tag = {}_Ok }}", c, c)
            }
            Type::Vec(inner) => format!("(Keel_Vec_{}){{0}}", inner.mangle()),
            Type::Tuple(_) => format!("({}){{0}}", self.ty(t)),
            Type::Slice(_) => format!("({}){{0}}", self.ty(t)),
            Type::Fn(..) => "NULL".to_string(),
            Type::Array(..) | Type::Void => "0".to_string(),
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        // `#line` привязывает ошибки C к строке Keel: gcc сообщает
        // `file.keel:строка`, а не строку сгенерированного C.
        self.out.borrow_mut().push_str(&format!(
            "#line {} \"{}\"\n",
            stmt.line(),
            self.cur_file
        ));
        match stmt {
            Stmt::Let { name, ty, init, .. } => {
                let t = match ty {
                    Some(t) => t.clone(),
                    None => self.types.get(&init.id).cloned().unwrap_or(Type::Int),
                };
                // Затенение: второе `let x` получает другое имя в C.
                let name = &self.fresh_var_name(name);
                match &t {
                    Type::Array(_, _) => {
                        let items = self.array_items(init);
                        self.out.borrow_mut().push_str(&format!(
                            "  {} = {{{}}};\n",
                            self.decl(&t, name),
                            items.join(", ")
                        ));
                    }
                    Type::Slice(_) => {
                        let init_ty = self.types.get(&init.id).cloned().unwrap_or(Type::Void);
                        let v = match init_ty {
                            Type::Array(_, n) => format!(
                                "({}){{ .data = {}, .len = {} }}",
                                self.ty(&t),
                                self.expr(init),
                                n
                            ),
                            _ => self.expr(init),
                        };
                        self.out.borrow_mut().push_str(&format!("  {} = {};\n", self.decl(&t, name), v));
                    }
                    Type::Named(_) => {
                        // Если инициализатор — вызов конструктора именно этой
                        // структуры, раскрываем в составной литерал.
                        // Иначе — обычное выражение (например, вызов функции,
                        // возвращающей структуру).
                        let is_ctor = match (&init.kind, name.as_str()) {
                            (ExprKind::Call { callee, .. }, n) => {
                                matches!(&callee.kind, ExprKind::Ident(c) if c == n)
                            }
                            _ => false,
                        };
                        if is_ctor {
                            let items = self.struct_fields(&t, init);
                            self.out.borrow_mut().push_str(&format!(
                                "  {} = {{{}}};\n",
                                self.decl(&t, name),
                                items.join(", ")
                            ));
                        } else {
                            self.out.borrow_mut().push_str(&format!(
                                "  {} = {};\n",
                                self.decl(&t, name),
                                self.expr(init)
                            ));
                        }
                    }
                    t => {
                        self.out.borrow_mut().push_str(&format!(
                            "  {} = {};\n",
                            self.decl(t, name),
                            self.expr(init)
                        ));
                    }
                }
            }
            Stmt::Assign { lhs, rhs } => {
                self.out.borrow_mut().push_str(&format!("  {} = {};\n", self.expr(lhs), self.expr(rhs)));
            }
            Stmt::Expr(e) => {
                let s = self.expr(e);
                if s != "0" && !s.is_empty() {
                    self.out.borrow_mut().push_str(&format!("  {};\n", s));
                }
            }
            Stmt::Return { value, .. } => {
                // Значение возврата и его тип: `return;` в main — это 0.
                let (v, t) = match value {
                    Some(e) => (self.expr(e), self.cur_ret.clone()),
                    None if self.in_main => ("0".to_string(), Type::Int),
                    None => (String::new(), Type::Void),
                };
                if self.cur_post.is_empty() {
                    match value {
                        Some(_) => self.out.borrow_mut().push_str(&format!("  return {};\n", v)),
                        None if self.in_main => self.out.borrow_mut().push_str("  return 0;\n"),
                        None => self.out.borrow_mut().push_str("  return;\n"),
                    }
                } else if t == Type::Void {
                    self.emit_post_checks();
                    self.out.borrow_mut().push_str("  return;\n");
                } else {
                    // Значение кладётся в переменную один раз: `post`
                    // может ссылаться на `result`, а выражение возврата
                    // не должно вычисляться дважды.
                    self.out.borrow_mut().push_str("  {\n");
                    self.out.borrow_mut().push_str(&format!(
                        "    {} = {};\n",
                        self.decl(&t, "__k_result"),
                        v
                    ));
                    self.emit_post_checks();
                    self.out.borrow_mut().push_str("    return __k_result;\n");
                    self.out.borrow_mut().push_str("  }\n");
                }
            }
            Stmt::Match { subject, arms, .. } => {
                // Субъект читается по значению: ссылка разыменовывается.
                let subj = self.value_expr(subject);
                let subj_ty = self.types.get(&subject.id).cloned().unwrap_or(Type::Int);
                let subj_ty = match subj_ty {
                    Type::Ref(inner) => *inner,
                    t => t,
                };
                // (C-имя типа, варианты (имя, payload), есть ли данные)
                let (cname, variants, is_payload) = match &subj_ty {
                    Type::Option(inner) => {
                        let c = format!("Keel_Option_{}", inner.mangle());
                        let vs = vec![
                            ("Some".to_string(), vec![(**inner).clone()]),
                            ("None".to_string(), Vec::new()),
                        ];
                        (c, vs, true)
                    }
                    Type::Result(ok, err) => {
                        let c = format!("Keel_Result_{}_{}", ok.mangle(), err.mangle());
                        let vs = vec![
                            ("Ok".to_string(), vec![(**ok).clone()]),
                            ("Err".to_string(), vec![(**err).clone()]),
                        ];
                        (c, vs, true)
                    }
                    Type::Named(n) => {
                        let e = self.enums.get(n);
                        let vs: Vec<(String, Vec<Type>)> = e
                            .map(|e| {
                                e.variants
                                    .iter()
                                    .map(|v| (v.name.clone(), v.payload.clone()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let payload = e
                            .map(|e| e.variants.iter().any(|v| !v.payload.is_empty()))
                            .unwrap_or(false);
                        (
                            self.ctype.get(n).cloned().unwrap_or_else(|| n.clone()),
                            vs,
                            payload,
                        )
                    }
                    _ => (String::new(), Vec::new(), false),
                };
                let tag = if is_payload {
                    format!("{}.tag", subj)
                } else {
                    subj.clone()
                };
                self.out.borrow_mut().push_str(&format!("  switch ({}) {{\n", tag));
                for arm in arms {
                    let case = if is_payload {
                        format!("{}_{}", cname, arm.variant)
                    } else {
                        format!("Keel_{}_{}", arm.ty, arm.variant)
                    };
                    self.out.borrow_mut().push_str(&format!("    case {}: {{\n", case));
                    // Привязки данных: `Some(x)` → `int64_t x = ...`.
                    let payload: Vec<Type> = variants
                        .iter()
                        .find(|(n, _)| n == &arm.variant)
                        .map(|(_, p)| p.clone())
                        .unwrap_or_default();
                    let saved: Vec<(String, Option<String>)> = arm
                        .bindings
                        .iter()
                        .map(|b| (b.clone(), self.var_names.borrow().get(b).cloned()))
                        .collect();
                    if is_payload {
                        for (i, b) in arm.bindings.iter().enumerate() {
                            let cname = self.fresh_var_name(b);
                            let ty = payload.get(i).cloned().unwrap_or(Type::Int);
                            let decl = self.decl(&ty, &cname);
                            self.out.borrow_mut().push_str(&format!(
                                "      {} = {}.data.{}._{};\n",
                                decl, subj, arm.variant, i
                            ));
                        }
                    }
                    for s in &arm.body.stmts {
                        self.emit_stmt(s);
                    }
                    self.out.borrow_mut().push_str("      break;\n");
                    self.out.borrow_mut().push_str("    }\n");
                    for (b, prev) in saved {
                        match prev {
                            Some(v) => {
                                self.var_names.borrow_mut().insert(b, v);
                            }
                            None => {
                                self.var_names.borrow_mut().remove(&b);
                            }
                        }
                    }
                }
                self.out.borrow_mut().push_str("  }\n");
            }
            Stmt::If { cond, then_block, else_block } => {
                self.out.borrow_mut()
                    .push_str(&format!("  if ({}) {{\n", self.expr(cond)));
                for s in &then_block.stmts {
                    self.emit_stmt(s);
                }
                self.out.borrow_mut().push_str("  }\n");
                if let Some(eb) = else_block {
                    self.out.borrow_mut().push_str("  else {\n");
                    for s in &eb.stmts {
                        self.emit_stmt(s);
                    }
                    self.out.borrow_mut().push_str("  }\n");
                }
            }
            Stmt::While { cond, body } => {
                self.out.borrow_mut()
                    .push_str(&format!("  while ({}) {{\n", self.expr(cond)));
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.out.borrow_mut().push_str("  }\n");
            }
            Stmt::DoWhile { body, cond } => {
                self.out.borrow_mut().push_str("  do {\n");
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.out.borrow_mut()
                    .push_str(&format!("  }} while ({});\n", self.expr(cond)));
            }
            Stmt::Break { .. } => {
                self.out.borrow_mut().push_str("  break;\n");
            }
            Stmt::Continue { .. } => {
                self.out.borrow_mut().push_str("  continue;\n");
            }
            Stmt::ForRange { var, start, end, body, .. } => {
                // Правая граница исключительная, как в Rust и Zeta:
                // `0..3` — это 0, 1, 2. Поэтому условие `<`.
                // Переменная цикла объявлена в самом `for` — иначе два
                // вложенных цикла с одинаковым именем конфликтуют в C.
                // В C область действия `for` охватывает всё тело, поэтому
                // внутренний цикл с тем же именем перетирает внешний. Чтобы
                // этого не было, внутренние циклы получают своё имя
                // (по номеру вложенности).
                let (cvar, _cstart) = self.loop_var(var);
                self.out.borrow_mut().push_str(&format!(
                    "  for ({} = {}; {} < {}; {}++) {{\n",
                    self.decl(&Type::Int, &cvar),
                    self.expr(start),
                    cvar,
                    self.expr(end),
                    cvar
                ));
                // На время тела цикла имя переменной в Keel отображается
                // в имя в C.
                self.loop_vars.insert(var.clone(), cvar);
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.loop_vars.remove(var);
                self.loop_depth -= 1;
                self.out.borrow_mut().push_str("  }\n");
            }
            Stmt::ForEach { var, iter, body, .. } => {
                // Обход массива: индексируемся, копия элемента попадает
                // в переменную цикла.
                let arr = self.expr(iter);
                let (elem_ty, len) = self.array_info(iter);
                let (cvar, _) = self.loop_var(var);
                self.out.borrow_mut().push_str(&format!(
                    "  for (int64_t __ki = 0; __ki < {}; __ki++) {{\n",
                    len
                ));
                self.out.borrow_mut().push_str(&format!(
                    "    {} = {}[__ki];\n",
                    self.decl(&elem_ty, &cvar),
                    arr
                ));
                self.loop_vars.insert(var.clone(), cvar);
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.loop_vars.remove(var);
                self.loop_depth -= 1;
                self.out.borrow_mut().push_str("  }\n");
            }
        }
    }

    /// Новое имя переменной в C с учётом затенений: первое объявление
    /// `x`, второе — `x__1`, третье — `x__2`. Иначе C скажет
    /// «redefinition». Имя запоминается в `var_names`, чтобы обращения
    /// к переменной шли к правильному объявлению.
    fn fresh_var_name(&self, name: &str) -> String {
        let mut counts = self.shadow_count.borrow_mut();
        let n = counts.get(name).copied().unwrap_or(0);
        counts.insert(name.to_string(), n + 1);
        let cname = if n == 0 {
            name.to_string()
        } else {
            format!("{}__{}", name, n)
        };
        self.var_names
            .borrow_mut()
            .insert(name.to_string(), cname.clone());
        cname
    }

    /// Имя переменной в C по её имени в Keel (с учётом затенений).
    fn var_name(&self, name: &str) -> String {
        self.var_names
            .borrow()
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    /// Имя переменной цикла в C. Вложенные циклы с одинаковым именем
    /// перетирают друг друга (в C область `for` — всё тело), поэтому
    /// каждый уровень вложенности получает свой суффикс.
    fn loop_var(&mut self, var: &str) -> (String, String) {
        let depth = self.loop_depth;
        self.loop_depth += 1;
        let name = if depth == 0 {
            var.to_string()
        } else {
            format!("{}__{}", var, depth)
        };
        (name, var.to_string())
    }

    /// Тип элемента и длина массива — для обхода `for ... in`.
    fn array_info(&self, iter: &Expr) -> (Type, usize) {
        let t = match self.types.get(&iter.id) {
            Some(Type::Array(elem, n)) => Some((elem.as_ref().clone(), *n)),
            Some(Type::Ref(inner)) => match inner.as_ref() {
                Type::Array(elem, n) => Some((elem.as_ref().clone(), *n)),
                _ => None,
            },
            _ => None,
        };
        t.unwrap_or((Type::Int, 0))
    }

    /// Элементы массива-литерала.
    ///
    /// В инициализации C требует константный инициализатор, поэтому
    /// вложенные массивы записываются фигурными скобками рекурсивно:
    /// `{{1, 2}, {3, 4}}`, а не `(int64_t[]){1, 2}`.
    fn array_items(&self, init: &Expr) -> Vec<String> {
        match &init.kind {
            ExprKind::Array(items) => items.iter().map(|x| self.array_init_elem(x)).collect(),
            ExprKind::Call { args, .. } => args.iter().map(|x| self.expr(x)).collect(),
            _ => vec![],
        }
    }

    /// Один элемент инициализатора массива: вложенный массив — `{...}`,
    /// всё остальное — обычное выражение.
    fn array_init_elem(&self, e: &Expr) -> String {
        if let ExprKind::Array(items) = &e.kind {
            let inner: Vec<String> = items.iter().map(|x| self.array_init_elem(x)).collect();
            return format!("{{{}}}", inner.join(", "));
        }
        self.expr(e)
    }

    /// Поля составного литерала структуры: `.hp = 42, .name = "orc"`.
    fn struct_fields(&self, t: &Type, init: &Expr) -> Vec<String> {
        let Type::Named(name) = t else { return vec![] };
        let Some(s) = self.structs.get(name) else { return vec![] };
        let args = match &init.kind {
            ExprKind::Call { args, .. } => args.clone(),
            _ => vec![],
        };
        let mut out = Vec::new();
        let prev = self.in_struct_init.replace(true);
        for (i, f) in s.fields.iter().enumerate() {
            let v = match args.get(i) {
                Some(a) => self.expr(a),
                None => "0".to_string(),
            };
            out.push(format!(".{} = {}", f.name, v));
        }
        self.in_struct_init.set(prev);
        out
    }

    /// Имя функции в C. Пользовательские имена получают префикс `k_`,
    /// чтобы не сталкиваться со стандартной библиотекой (`remove` из
    /// stdio.h, `index` из strings.h и т.д.). `main` остаётся как есть —
    /// это точка входа C.
    ///
    /// Метод из `impl`-блока получает имя `k_Тип_метод`, чтобы методы
    /// разных типов с одинаковым именем не сталкивались.
    fn fname(&self, name: &str) -> String {
        if name == "main" {
            name.to_string()
        } else if let Some((ty, m)) = name.split_once("::") {
            // Ассоциированная функция: `Point::new` → `k_Point_new`.
            format!("k_{}_{}", ty, m)
        } else {
            format!("k_{}", name)
        }
    }

    /// Имя функции в C с учётом типа-владельца.
    fn fname_of(&self, f: &FuncDecl) -> String {
        match &f.owner {
            Some(ty) => format!("k_{}_{}", ty, f.name),
            None => self.fname(&f.name),
        }
    }

    fn func_params(&self, name: &str) -> Option<&[Param]> {
        for item in &self.prog.items {
            match item {
                Item::Func(f) if f.name == name => return Some(&f.params),
                Item::Impl(im) => {
                    for f in &im.funcs {
                        if f.name == name {
                            return Some(&f.params);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Выражение в «значенческом» контексте: если тип — ссылка,
    /// автоматически разыменовываем (`*r`).
    fn value_expr(&self, e: &Expr) -> String {
        let base = self.expr(e);
        match self.types.get(&e.id) {
            Some(Type::Ref(_)) => format!("(*{})", base),
            _ => base,
        }
    }

    fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(n) => format!("INT64_C({})", n),
            ExprKind::Float(v) => format!("{:?}", v),
            ExprKind::Bool(b) => format!("{}", b),
            ExprKind::Str(s) => {
                let mut out = String::from("\"");
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '\r' => out.push_str("\\r"),
                        other => out.push(other),
                    }
                }
                out.push('"');
                out
            }
            ExprKind::Ident(name) if name == "None" => {
                let t = self
                    .types
                    .get(&e.id)
                    .cloned()
                    .unwrap_or(Type::Option(Box::new(Type::Int)));
                if let Type::Option(inner) = t {
                    let c = format!("Keel_Option_{}", inner.mangle());
                    return format!("({}){{ .tag = {}_None }}", c, c);
                }
                "0".to_string()
            }
            ExprKind::Ident(name) => match self.loop_vars.get(name) {
                Some(cname) => cname.clone(),
                None => self.var_name(name),
            },
            ExprKind::Variant { ty, name } => {
                if let Some(e) = self.enums.get(ty) {
                    if e.variants.iter().any(|v| !v.payload.is_empty()) {
                        let c = self.ctype.get(ty).cloned().unwrap_or_else(|| ty.clone());
                        return format!("({}){{ .tag = Keel_{}_{} }}", c, ty, name);
                    }
                }
                format!("Keel_{}_{}", ty, name)
            }
            ExprKind::StructInit { name, fields } => {
                let c = self.ctype.get(name).cloned().unwrap_or_else(|| name.clone());
                let inits: Vec<String> = fields
                    .iter()
                    .map(|(f, v)| format!(".{} = {}", f, self.expr(v)))
                    .collect();
                format!("({}){{ {} }}", c, inits.join(", "))
            }
            ExprKind::Tuple(items) => {
                let t = self.types.get(&e.id).cloned().unwrap_or(Type::Void);
                let c = self.ty(&t);
                let inits: Vec<String> = items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("._{} = {}", i, self.expr(v)))
                    .collect();
                format!("({}){{ {} }}", c, inits.join(", "))
            }
            ExprKind::Field { obj, name } => {
                let base = self.expr(obj);
                let base_ty = self.types.get(&obj.id);
                // Если base — ссылка (указатель), используем ->.
                match base_ty {
                    Some(Type::Tuple(_)) => format!("{}._{}", base, name),
                    Some(Type::Ref(inner)) if matches!(inner.as_ref(), Type::Tuple(_)) => {
                        format!("{}->_{}", base, name)
                    }
                    Some(Type::Ref(_)) => format!("{}->{}", base, name),
                    _ => format!("{}.{}", base, name),
                }
            }
            ExprKind::Index { obj, idx } => {
                let base = self.expr(obj);
                let base_ty = self.types.get(&obj.id);
                let i = self.expr(idx);
                let (data, len) = match base_ty {
                    // Ссылка на массив (T (*)[N]): индексация в C дала бы
                    // сам массив, а не элемент — нужно разыменовать.
                    Some(Type::Ref(inner)) if matches!(inner.as_ref(), Type::Array(..)) => {
                        let Type::Array(_, n) = inner.as_ref() else { unreachable!() };
                        (format!("(*{})", base), n.to_string())
                    }
                    Some(Type::Array(_, n)) => (base.clone(), n.to_string()),
                    Some(Type::Vec(_)) => (format!("{}.data", base), format!("{}.len", base)),
                    Some(Type::Slice(_)) => (format!("{}.data", base), format!("{}.len", base)),
                    Some(Type::Ref(inner)) if matches!(inner.as_ref(), Type::Vec(_)) => {
                        (format!("{}->data", base), format!("{}->len", base))
                    }
                    _ => (base.clone(), String::new()),
                };
                if len.is_empty() {
                    format!("{}[{}]", base, i)
                } else {
                    // shortcut: индекс и база вычисляются дважды; для
                    // выражений с побочными эффектами нужен временный —
                    // когда такие индексы появятся.
                    format!("{}[k_index_check({}, {}, {})]", data, i, len, e.line)
                }
            }
            ExprKind::Call { callee, args } => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return "0".to_string();
                };
                // Вызов замыкания: `f(args)`, где `f` — локальная типа `fn(...)`.
                if let Some(Type::Fn(..)) = self.types.get(&callee.id) {
                    let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                    return format!("{}({})", self.var_name(name), a.join(", "));
                }
                // Встроенный `Vec::new()`.
                if name == "Vec::new" {
                    let t = self
                        .types
                        .get(&e.id)
                        .cloned()
                        .unwrap_or(Type::Vec(Box::new(Type::Int)));
                    if let Type::Vec(elem) = t {
                        let c = format!("Keel_Vec_{}", elem.mangle());
                        return format!("({}){{0}}", c);
                    }
                }
                // Встроенные конструкторы Option/Result.
                if name == "Some" {
                    let t = self
                        .types
                        .get(&e.id)
                        .cloned()
                        .unwrap_or(Type::Option(Box::new(Type::Int)));
                    if let Type::Option(inner) = t {
                        let c = format!("Keel_Option_{}", inner.mangle());
                        let v = args
                            .first()
                            .map(|a| self.expr(a))
                            .unwrap_or_else(|| "0".to_string());
                        return format!(
                            "({}){{ .tag = {}_Some, .data.Some = {{ ._0 = {} }} }}",
                            c, c, v
                        );
                    }
                }
                if name == "Ok" || name == "Err" {
                    let t = self.types.get(&e.id).cloned().unwrap_or(Type::Result(
                        Box::new(Type::Int),
                        Box::new(Type::Str),
                    ));
                    if let Type::Result(ok, err) = t {
                        let c = format!("Keel_Result_{}_{}", ok.mangle(), err.mangle());
                        let v = args
                            .first()
                            .map(|a| self.expr(a))
                            .unwrap_or_else(|| "0".to_string());
                        let variant = if name == "Ok" { "Ok" } else { "Err" };
                        return format!(
                            "({}){{ .tag = {}_{}, .data.{} = {{ ._0 = {} }} }}",
                            c, c, variant, variant, v
                        );
                    }
                }
                // Конструктор варианта enum: `Option::Some(5)`.
                if let Some((ty, variant)) = name.split_once("::") {
                    if let Some(e) = self.enums.get(ty) {
                        if let Some(v) = e.variants.iter().find(|v| v.name == variant) {
                            let c = self.ctype.get(ty).cloned().unwrap_or_else(|| ty.to_string());
                            if v.payload.is_empty() {
                                return format!("({}){{ .tag = Keel_{}_{} }}", c, ty, variant);
                            }
                            let mut fields = Vec::new();
                            for (i, a) in args.iter().enumerate() {
                                fields.push(format!("._{} = {}", i, self.expr(a)));
                            }
                            return format!(
                                "({}){{ .tag = Keel_{}_{}, .data.{} = {{ {} }} }}",
                                c,
                                ty,
                                variant,
                                variant,
                                fields.join(", ")
                            );
                        }
                    }
                }
                if name == "print" || name == "println" {
                    if args.is_empty() {
                        // `println()` без аргументов — просто перевод строки.
                        if name == "println" {
                            return "printf(\"\\n\")".to_string();
                        }
                        return String::new();
                    }
                    let nl = if name == "println" { "\\n" } else { "" };
                    let last = args.len() - 1;
                    let mut parts = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        let mut t = self.types.get(&a.id).cloned().unwrap_or(Type::Int);
                        let mut v = self.expr(a);
                        // Ссылка печатается по значению: авторазыменование.
                        if let Type::Ref(inner) = t.clone() {
                            v = format!("(*{})", v);
                            t = *inner;
                        }
                        // Перевод строки — только у последнего аргумента.
                        let tail = if i == last { nl } else { "" };
                        parts.push(match t {
                            Type::Str => format!("printf(\"%s{}\", {})", tail, v),
                            Type::Float => format!("printf(\"%g{}\", {})", tail, v),
                            Type::Bool => {
                                format!("printf(\"%s{}\", {} ? \"true\" : \"false\")", tail, v)
                            }
                            _ => format!("printf(\"%lld{}\", (long long){})", tail, v),
                        });
                    }
                    // Несколько аргументов склеиваются оператором `,`:
                    // каждый `printf` возвращает int, но результат
                    // выражения всё равно не используется.
                    return parts.join(", ");
                }
                // `read_line` — строка из stdin. NULL при EOF превращаем
                // в пустую строку: в языке нет Option (волна 8).
                if name == "read_line" {
                    return "k_read_line_or_empty()".to_string();
                }
                // `read_int` — целое из stdin.
                if name == "read_int" {
                    return "k_read_int()".to_string();
                }
                // `argc` — сколько аргументов получила программа.
                if name == "argc" {
                    return "k_argc".to_string();
                }
                // `argv(i)` — i-й аргумент. Выход за границы дал бы
                // NULL и падение при печати, поэтому подставляем
                // пустую строку.
                if name == "argv" {
                    let i = args.first().map(|a| self.expr(a)).unwrap_or_else(|| "0".to_string());
                    return format!("(k_argv[{0}] ? k_argv[{0}] : \"\")", i);
                }
                // `open(path, mode)` — открывает файл, возвращает fd или -1.
                if name == "open" {
                    let path = args
                        .first()
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "\"\"".to_string());
                    let mode = args
                        .get(1)
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "\"r\"".to_string());
                    return format!("k_open({}, {})", path, mode);
                }
                // `read(fd)` — читает файл целиком в строку.
                if name == "read" {
                    let fd = args
                        .first()
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "0".to_string());
                    return format!("k_read({})", fd);
                }
                // `write(fd, s)` — пишет строку в файл.
                if name == "write" {
                    let fd = args
                        .first()
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "0".to_string());
                    let s = args
                        .get(1)
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "\"\"".to_string());
                    return format!("k_write({}, {})", fd, s);
                }
                // `close(fd)` — закрывает файл.
                if name == "close" {
                    let fd = args
                        .first()
                        .map(|a| self.expr(a))
                        .unwrap_or_else(|| "0".to_string());
                    return format!("k_close({})", fd);
                }
                // `len` — длина строки или массива. Для массива размер
                // известен из типа и подставляется числом; для строки
                // считается `strlen` в рантайме.
                if name == "len" {
                    let Some(a) = args.first() else {
                        return "0".to_string();
                    };
                    let t = self.types.get(&a.id).cloned().unwrap_or(Type::Int);
                    let inner = match &t {
                        Type::Ref(inner) => inner.as_ref().clone(),
                        other => other.clone(),
                    };
                    return match inner {
                        Type::Array(_, n) => n.to_string(),
                        Type::Vec(_) => {
                            let v = match t {
                                Type::Ref(_) => format!("{}->len", self.expr(a)),
                                _ => format!("{}.len", self.expr(a)),
                            };
                            format!("(long long){}", v)
                        }
                        Type::Slice(_) => {
                            let v = match t {
                                Type::Ref(_) => format!("{}->len", self.expr(a)),
                                _ => format!("{}.len", self.expr(a)),
                            };
                            format!("(long long){}", v)
                        }
                        _ => {
                            let v = match t {
                                Type::Ref(_) => format!("(*{})", self.expr(a)),
                                _ => self.expr(a),
                            };
                            format!("(long long)strlen({})", v)
                        }
                    };
                }
                // Конструктор структуры.
                if let Some(s) = self.structs.get(name) {
                    let cname = self.ctype.get(name).cloned().unwrap_or_else(|| name.clone());
                    // Если в поле лежит массив, в C нельзя написать
                    // `.f = arr` — массив не копируется в инициализатор.
                    // Для таких структур генерируется отдельная функция
                    // `k_new_Имя`, которая копирует поле через memcpy.
                    let has_array = s.fields.iter().any(|f| matches!(f.ty, Type::Array(_, _)));
                    if has_array {
                        let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                        return format!("k_new_{}({})", name, a.join(", "));
                    }
                    let mut items = Vec::new();
                    let prev = self.in_struct_init.replace(true);
                    for (i, f) in s.fields.iter().enumerate() {
                        let v = match args.get(i) {
                            Some(a) => self.expr(a),
                            None => "0".to_string(),
                        };
                        items.push(format!(".{} = {}", f.name, v));
                    }
                    self.in_struct_init.set(prev);
                    return format!("({}){{{}}}", cname, items.join(", "));
                }
                let params = self.func_params(name);
                let a: Vec<String> = args
                    .iter()
                    .enumerate()
                    .map(|(i, x)| {
                        let v = self.expr(x);
                        if let Some(p) = params.and_then(|ps| ps.get(i)) {
                            if let Type::Slice(inner) = &p.ty {
                                let at = self.types.get(&x.id).cloned().unwrap_or(Type::Void);
                                if let Type::Array(_, n) = at {
                                    return format!(
                                        "(Keel_Slice_{}){{ .data = {}, .len = {} }}",
                                        inner.mangle(),
                                        v,
                                        n
                                    );
                                }
                            }
                        }
                        v
                    })
                    .collect();
                format!("{}({})", self.fname(name), a.join(", "))
            }
            ExprKind::MethodCall { obj, name, args } => {
                // Метод вызывается как `k_Тип_метод(получатель, args...)`.
                let obj_ty = self.types.get(&obj.id).map(|t| t.deref().clone());
                if let Some(Type::Vec(elem)) = &obj_ty {
                    let m = elem.mangle();
                    let recv = match self.types.get(&obj.id) {
                        Some(Type::Ref(_)) => self.expr(obj),
                        _ => format!("(&{})", self.expr(obj)),
                    };
                    return match name.as_str() {
                        "push" => format!("k_vec_{}_push({}, {})", m, recv, self.expr(&args[0])),
                        "len" => format!("k_vec_{}_len({})", m, recv),
                        "get" => format!("k_vec_{}_get({}, {})", m, recv, self.expr(&args[0])),
                        "pop" => format!("k_vec_{}_pop({})", m, recv),
                        "free" => format!("k_vec_{}_free({})", m, recv),
                        _ => "0".to_string(),
                    };
                }
                let ty_name = match obj_ty {
                    Some(Type::Named(n)) => n,
                    _ => String::new(),
                };
                // Если метод ждёт ссылку, а получатель — значение,
                // передаём адрес: `&p` → `(&p)`. Решение принято
                // проверщиком и записано по id выражения.
                let take_ref = self.method_recv_ref.get(&e.id).copied().unwrap_or(false);
                let recv = if take_ref {
                    format!("(&{})", self.expr(obj))
                } else {
                    self.expr(obj)
                };
                let mut a: Vec<String> = vec![recv];
                a.extend(args.iter().map(|x| self.expr(x)));
                format!("k_{}_{}({})", ty_name, name, a.join(", "))
            }
            ExprKind::Unary { op, e } => match op {
                UnOp::Neg => format!("(-{})", self.value_expr(e)),
                UnOp::Not => format!("(!{})", self.value_expr(e)),
            },
            ExprKind::Binary { op, l, r } => {
                let (ls, rs) = (self.value_expr(l), self.value_expr(r));
                // Строки сравниваются по содержимому, а не по адресу.
                let both_str = matches!(self.types.get(&l.id), Some(Type::Str))
                    && matches!(self.types.get(&r.id), Some(Type::Str));
                if both_str {
                    let cmp = match op {
                        BinOp::Eq => "==",
                        BinOp::Ne => "!=",
                        BinOp::Lt => "<",
                        BinOp::Gt => ">",
                        BinOp::Le => "<=",
                        BinOp::Ge => ">=",
                        _ => "",
                    };
                    if !cmp.is_empty() {
                        return format!("(strcmp({}, {}) {} 0)", ls, rs, cmp);
                    }
                    // Конкатенация: результат — новая строка в куче.
                    if matches!(op, BinOp::Add) {
                        return format!("k_str_concat({}, {})", ls, rs);
                    }
                }
                let opstr = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Rem => "%",
                    BinOp::Eq => "==",
                    BinOp::Ne => "!=",
                    BinOp::Lt => "<",
                    BinOp::Gt => ">",
                    BinOp::Le => "<=",
                    BinOp::Ge => ">=",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                };
                format!("({} {} {})", ls, opstr, rs)
            }
            ExprKind::Closure { params, body } => {
                let name = format!(
                    "closure_{}",
                    self.closures.borrow().len() + self.pending_closures.borrow().len()
                );
                let ret = self.types.get(&e.id).cloned().unwrap_or(Type::Void);
                self.pending_closures
                    .borrow_mut()
                    .push((name.clone(), params.clone(), body.clone(), ret));
                name
            }
            ExprKind::If { cond, then_expr, else_expr } => {
                // Тернарный оператор C: значение — результат выбранной
                // ветви, типы у обеих ветвей совпадают (проверил checker).
                format!(
                    "({} ? {} : {})",
                    self.value_expr(cond),
                    self.value_expr(then_expr),
                    self.value_expr(else_expr)
                )
            }
            ExprKind::Array(items) => {
                // В выражении литерал массива — это составной литерал C
                // `(T[]){...}`. Голые `{...}` допустимы только в
                // инициализации, но не как аргумент вызова.
                //
                // Но в инициализации структуры (`World { entities = [...] }`)
                // C требует константный инициализатор, а составной литерал
                // им не является. Поэтому там оставляем `{...}`.
                if self.in_struct_init.get() {
                    let elems: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                    return format!("{{{}}}", elems.join(", "));
                }
                let elems: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                // Тип элемента для составного литерала C: `(T[]){...}`.
                // Для вложенного массива T сам является массивом, и в C
                // это записывается как `T[inner]`, а не `T`.
                let elem_ty = match self.types.get(&e.id) {
                    Some(Type::Array(t, _)) => self.array_literal_ty(t),
                    _ => "int64_t".to_string(),
                };
                format!("({}[]){{{}}}", elem_ty, elems.join(", "))
            }
            ExprKind::Ref(inner) => {
                // &expr → &(expr). Если expr — уже ссылка, разыменовываем.
                let base = self.expr(inner);
                match self.types.get(&inner.id) {
                    Some(Type::Ref(_)) => format!("(&(*{}))", base),
                    _ => format!("(&{})", base),
                }
            }
        }
    }
}

/// Упоминает ли оператор имя (для liveness `Vec`).
fn stmt_mentions(s: &Stmt, name: &str) -> bool {
    match s {
        Stmt::Let { init, .. } => crate::checker::mentions_ident(init, name),
        Stmt::Assign { lhs, rhs } => {
            crate::checker::mentions_ident(lhs, name) || crate::checker::mentions_ident(rhs, name)
        }
        Stmt::Expr(e) => crate::checker::mentions_ident(e, name),
        Stmt::Return { value, .. } => value
            .as_ref()
            .is_some_and(|e| crate::checker::mentions_ident(e, name)),
        Stmt::If { cond, then_block, else_block } => {
            crate::checker::mentions_ident(cond, name)
                || block_mentions(then_block, name)
                || else_block.as_ref().is_some_and(|b| block_mentions(b, name))
        }
        Stmt::While { cond, body } => {
            crate::checker::mentions_ident(cond, name) || block_mentions(body, name)
        }
        Stmt::DoWhile { body, cond } => {
            block_mentions(body, name) || crate::checker::mentions_ident(cond, name)
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => false,
        Stmt::ForRange { start, end, body, .. } => {
            crate::checker::mentions_ident(start, name)
                || crate::checker::mentions_ident(end, name)
                || block_mentions(body, name)
        }
        Stmt::ForEach { iter, body, .. } => {
            crate::checker::mentions_ident(iter, name) || block_mentions(body, name)
        }
        Stmt::Match { subject, arms, .. } => {
            crate::checker::mentions_ident(subject, name)
                || arms.iter().any(|a| block_mentions(&a.body, name))
        }
    }
}

fn block_mentions(b: &Block, name: &str) -> bool {
    b.stmts.iter().any(|s| stmt_mentions(s, name))
}
