//! Проверщик заимствований Keel (group borrowing, упрощённый).
//!
//! Модель:
//! - каждое значение принадлежит ровно одному владельцу (single ownership);
//! - каждая ссылка помнит путь, на который указывает;
//! - изменение объекта инвалидирует ссылки, указывающие ВНУТРЬ него;
//! - `mut`-контракт функции описывает, что она модифицирует.
//!
//! Инвалидация бывает двух видов:
//! - **Replace** — значение по пути заменено целиком (`x.f = v`): инвалидируются
//!   ссылки на сам путь и на всё внутри него;
//! - **MutateInside** — содержимое изменено, объект остался на месте
//!   (`vec.push(..)`): инвалидируются только ссылки строго внутри.

use crate::ast::*;
use std::collections::HashMap;

/// Склонение слова «аргумент» по числу: 1 аргумент, 2 аргумента,
/// 5 аргументов. Без этого в сообщении появляется «ждёт 1 аргументов».
fn аргументов(n: usize) -> String {
    let word = if n % 10 == 1 && n % 100 != 11 {
        "аргумент"
    } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
        "аргумента"
    } else {
        "аргументов"
    };
    format!("{} {}", n, word)
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub line: usize,
    pub msg: String,
    /// Имя файла, из которого пришла ошибка. Пусто при `compile(src)`.
    pub file: String,
}

impl Diagnostic {
    fn new(line: usize, msg: String) -> Self {
        Diagnostic { line, msg, file: String::new() }
    }
}

/// Список диагностик с текущим файлом: `push` проставляет имя файла,
/// чтобы не передавать его в каждый из сотни вызовов `Diagnostic::new`.
struct Diags {
    items: Vec<Diagnostic>,
    file: String,
}

impl Diags {
    fn new() -> Diags {
        Diags { items: Vec::new(), file: String::new() }
    }

    fn push(&mut self, mut d: Diagnostic) {
        d.file = self.file.clone();
        self.items.push(d);
    }
}

// ============================================================
// Локальные переменные
// ============================================================

/// Ссылка на локальную переменную, спрятанная внутри значения.
///
/// Первый элемент — путь ВНУТРИ значения (например `[Field("r")]` для
/// поля `r` структуры), второй — путь, на который эта ссылка указывает.
/// Нужен, чтобы поймать `let h = H(&i); return h;`: сам `h` — не ссылка,
/// но внутри него лежит ссылка на локальную `i`.
type Escape = (Vec<Seg>, Path);

#[derive(Clone)]
struct Local {
    /// Уникальный идентификатор локальной переменной.
    uid: u32,
    ty: Type,
    /// Путь, по которому лежит само значение (root = uid).
    path: Path,
    /// Если это ссылка — путь, на который она указывает.
    ref_target: Option<Path>,
    /// Идентификатор записи в `refs`, если это ссылка.
    ref_uid: Option<u32>,
    /// Значение было перемещено — использовать нельзя.
    moved: bool,
    /// Переменную разрешено переприсваивать.
    is_mut: bool,
    /// Ссылки на локальные переменные, лежащие внутри значения.
    /// Пусто у всего, что не содержит ссылок на локальные.
    escapes: Vec<Escape>,
}

#[derive(Clone)]
struct RefLocal {
    uid: u32,
    /// Пути, на которые может указывать ссылка. У точной ссылки —
    /// один; у результата вызова/`if` — все ref-аргументы (ветви).
    targets: Vec<Path>,
    dead: bool,
    /// Строка, где ссылка создана (0 — параметр: у него нет строки).
    created_line: usize,
    /// Строка и причина, погасившие ссылку.
    killed: Option<(usize, String)>,
    /// Цель известна неточно (результат вызова или `if`): гасим при
    /// любом изменении внутри цели.
    imprecise: bool,
}

// ============================================================
// Проверщик
// ============================================================

pub struct Checker {
    structs: HashMap<String, StructDecl>,
    /// Объявления enum: имя → варианты.
    enums: HashMap<String, EnumDecl>,
    funcs: HashMap<String, FuncInfo>,
    /// Методы: (имя типа, имя метода) → сигнатура. Вызов `x.m(...)`
    /// ищется по типу получателя `x`.
    methods: HashMap<(String, String), FuncInfo>,
    /// Строка объявления каждой функции — для диагностик.
    /// Строка объявления функции — для диагностик о повторных именах.
    func_lines: HashMap<String, usize>,
    /// Файл объявления функции — для диагностик о повторных именах.
    func_files: HashMap<String, String>,
    /// uid -> имя, для читаемых сообщений об ошибках.
    names: HashMap<u32, String>,
    next_uid: u32,
    /// Активные ссылки в текущей функции.
    refs: Vec<RefLocal>,
    /// Пути, изменённые в текущей функции (для проверки mut-контракта).
    modified: Vec<(Path, bool)>,
    /// Тип каждого выражения (по id) — нужен кодогенератору.
    pub expr_types: HashMap<u32, Type>,
    /// Разрешённые типы параметров: (функция, параметр) → тип.
    /// Нужны кодогенератору: тип параметра-внутри выводится из пути.
    pub param_types: HashMap<(String, String), Type>,
    /// id выражения `x.method(...)` → нужно ли передать получателя
    /// ссылкой. Метод, объявленный как `self &T`, при вызове на значении
    /// получает адрес; кодогенератор должен знать это по id выражения.
    pub method_recv_ref: HashMap<u32, bool>,
    /// Конкретные инстанциации встроенных `Option<T>` — для кодогенератора.
    pub option_insts: Vec<Type>,
    /// Конкретные инстанциации встроенных `Result<T, E>`.
    pub result_insts: Vec<(Type, Type)>,
    /// Конкретные инстанциации встроенных `Vec<T>`.
    pub vec_insts: Vec<Type>,
    /// Конкретные типы кортежей — для кодогенератора.
    pub tuple_insts: Vec<Type>,
    /// Конкретные типы срезов — для кодогенератора.
    pub slice_insts: Vec<Type>,
    /// `Vec`-локальные, которые надо освободить в конце функции:
    /// функция → [(имя, тип)]. Не включает перемещённые и явно
    /// освобождённые (`v.free()`).
    pub vec_drops: HashMap<String, Vec<(String, Type, Vec<String>)>>,
    diags: Diags,
}

#[derive(Clone)]
struct FuncInfo {
    params: Vec<Param>,
    ret: Type,
    mut_paths: Vec<MutPath>,
    /// Файл объявления — для проверки видимости.
    file: String,
    /// Видна ли функция из других файлов (`pub` или метод типа).
    public: bool,
}

impl Checker {
    pub fn new() -> Checker {
        Checker {
            structs: HashMap::new(),
            enums: HashMap::new(),
            funcs: HashMap::new(),
            methods: HashMap::new(),
            func_lines: HashMap::new(),
            func_files: HashMap::new(),
            names: HashMap::new(),
            next_uid: 1,
            refs: Vec::new(),
            modified: Vec::new(),
            expr_types: HashMap::new(),
            param_types: HashMap::new(),
            method_recv_ref: HashMap::new(),
            option_insts: Vec::new(),
            result_insts: Vec::new(),
            vec_insts: Vec::new(),
            tuple_insts: Vec::new(),
            slice_insts: Vec::new(),
            vec_drops: HashMap::new(),
            diags: Diags::new(),
        }
    }

    pub fn check(&mut self, program: &Program) -> Vec<Diagnostic> {
        // Сначала собираем объявления, чтобы функции могли ссылаться друг на друга.
        // Повторные имена — ошибка: в C они дают «redefinition» в
        // сгенерированном коде, где причина не видна, либо молча
        // перекрывают друг друга.
        for item in &program.items {
            self.diags.file = match item {
                Item::Struct(s) => s.file.clone(),
                Item::Enum(e) => e.file.clone(),
                Item::Func(f) => f.file.clone(),
                Item::Impl(_) | Item::Import(_) => String::new(),
            };
            match item {
                Item::Struct(s) => {
                    if let Some(prev) = self.structs.get(&s.name) {
                        self.diags.push(Diagnostic::new(
                            s.line,
                            format!(
                                "структура `{}` объявлена дважды (первое объявление — строка {})",
                                s.name, prev.line
                            ),
                        ));
                    }
                    if let Some(prev) = self.enums.get(&s.name) {
                        self.diags.push(Diagnostic::new(
                            s.line,
                            format!(
                                "имя `{}` занято и enum, и структурой (enum — строка {})",
                                s.name, prev.line
                            ),
                        ));
                    }
                    self.structs.insert(s.name.clone(), s.clone());
                }
                Item::Enum(e) => {
                    if let Some(prev) = self.enums.get(&e.name) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "enum `{}` объявлен дважды (первое объявление — строка {})",
                                e.name, prev.line
                            ),
                        ));
                    }
                    if let Some(prev) = self.structs.get(&e.name) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "имя `{}` занято и структурой, и enum (структура — строка {})",
                                e.name, prev.line
                            ),
                        ));
                    }
                    self.enums.insert(e.name.clone(), e.clone());
                }
                Item::Func(f) => {
                    self.declare_func(f, None, f.public);
                }
                Item::Impl(im) => {
                    for f in &im.funcs {
                        self.diags.file = f.file.clone();
                        if Self::is_method(f) {
                            // Метод: ключ — (тип, имя). Вызов идёт как
                            // `x.name(...)`, поэтому ищем по типу получателя.
                            let key = (im.ty.clone(), f.name.clone());
                            if self.methods.contains_key(&key) {
                                self.diags.push(Diagnostic::new(
                                    f.line,
                                    format!(
                                        "метод `{}` у типа `{}` объявлен дважды",
                                        f.name, im.ty
                                    ),
                                ));
                            }
                            self.methods.insert(
                                key,
                                FuncInfo {
                                    params: f.params.clone(),
                                    ret: f.ret.clone(),
                                    mut_paths: f.mut_paths.clone(),
                                    file: f.file.clone(),
                                    public: im.public,
                                },
                            );
                        } else {
                            self.declare_func(f, Some(&im.ty), im.public);
                        }
                    }
                }
                // Импорты обрабатывает загрузчик модулей: до проверщика
                // они не доходят.
                Item::Import(_) => {}
            }
        }

        // Собираем инстанциации встроенных обобщённых типов из объявлений:
        // `Vec<int>` в параметре или поле должен быть сгенерирован, даже
        // если конструктор `Vec::new()` в этом файле не встречается.
        for item in &program.items {
            match item {
                Item::Struct(s) => {
                    for f in &s.fields {
                        self.record_type_insts(&f.ty);
                    }
                }
                Item::Func(f) => {
                    for p in &f.params {
                        self.record_type_insts(&p.ty);
                    }
                    self.record_type_insts(&f.ret);
                }
                Item::Impl(im) => {
                    for f in &im.funcs {
                        for p in &f.params {
                            self.record_type_insts(&p.ty);
                        }
                        self.record_type_insts(&f.ret);
                    }
                }
                _ => {}
            }
        }

        // Имя, занятое и структурой, и функцией, — почти всегда ошибка:
        // `User(...)` разбирается как конструктор структуры, и функция
        // становится недостижимой. Лучше сказать об этом прямо.
        for f in self.funcs.keys() {
            if let Some((ty, _)) = f.split_once("::") {
                // Ассоциированная функция: `Тип::имя` — конфликт только
                // если есть структура с именем `Тип::имя` (невозможно).
                let _ = ty;
                continue;
            }
            if self.structs.contains_key(f) {
                let line = self
                    .func_lines
                    .get(f)
                    .copied()
                    .unwrap_or(0);
                self.diags.file = self.func_files.get(f).cloned().unwrap_or_default();
                self.diags.push(Diagnostic::new(
                    line,
                    format!(
                        "имя `{}` занято и структурой, и функцией: вызов `{}(...)` \
                         всегда означает конструктор структуры, поэтому функция \
                         недостижима — переименуйте одно из двух",
                        f, f
                    ),
                ));
            }
        }

        // Данные вариантов: пока только копируемые типы. Иначе
        // деструктуризация в `match` двигала бы значение из enum, а
        // enum перестал бы быть копируемым.
        let enums: Vec<EnumDecl> = self.enums.values().cloned().collect();
        for e in &enums {
            self.diags.file = e.file.clone();
            for v in &e.variants {
                for t in &v.payload {
                    if !self.payload_type_ok(t) {
                        self.diags.push(Diagnostic::new(
                            v.line,
                            format!(
                                "вариант `{}::{}`: тип `{}` в данных пока не поддерживается — \
                                 допустимы int, bool, string и enum",
                                e.name,
                                v.name,
                                t.name()
                            ),
                        ));
                    }
                }
            }
        }
        // Рекурсивный enum бесконечен по размеру: `enum E { A(E) }`.
        for e in &enums {
            self.diags.file = e.file.clone();
            let mut visiting = Vec::new();
            if self.enum_has_cycle(&e.name, &mut visiting) {
                self.diags.push(Diagnostic::new(
                    e.line,
                    format!("enum `{}` рекурсивен: вариант содержит сам себя", e.name),
                ));
            }
        }

        // Затем проверяем тела.
        for item in &program.items {
            match item {
                Item::Func(f) => {
                    self.diags.file = f.file.clone();
                    self.check_func(f);
                }
                Item::Impl(im) => {
                    for f in &im.funcs {
                        self.diags.file = f.file.clone();
                        self.check_func(f);
                    }
                }
                Item::Struct(_) => {}
                Item::Enum(_) => {}
                // Импорты обрабатывает загрузчик модулей: до
                // проверщика и генератора они не доходят.
                Item::Import(_) => {}
            }
        }

        // Дедуп и сортировка: двойной проход по телам циклов даёт
        // повторы, а обход HashMap — недетерминированный порядок.
        let mut items = self.diags.items.clone();
        items.sort_by(|a, b| {
            (a.file.clone(), a.line, a.msg.clone()).cmp(&(b.file.clone(), b.line, b.msg.clone()))
        });
        items.dedup_by(|a, b| a.line == b.line && a.msg == b.msg && a.file == b.file);
        items
    }

    /// Метод — функция с первым параметром `self`.
    fn is_method(f: &FuncDecl) -> bool {
        f.params.first().map(|p| p.name == "self").unwrap_or(false)
    }

    /// Регистрирует функцию в таблице, сообщая о повторном имени.
    /// `owner` — имя типа для ассоциированной функции из `impl`-блока.
    ///
    /// Повторное имя — не мелочь: в C это «redefinition of 'k_f'» в
    /// сгенерированном коде, где причина не видна. Два `main` — тем
    /// более: C соберёт только один, и поведение зависит от порядка.
    fn declare_func(&mut self, f: &FuncDecl, owner: Option<&str>, public: bool) {
        let key = match owner {
            Some(ty) => format!("{}::{}", ty, f.name),
            None => f.name.clone(),
        };
        if let Some(prev) = self.func_lines.get(&key) {
            let what = if f.name == "main" {
                format!("функция `main` объявлена дважды (первое объявление — строка {})", prev)
            } else {
                format!(
                    "функция `{}` объявлена дважды (первое объявление — строка {})",
                    key, prev
                )
            };
            self.diags.push(Diagnostic::new(f.line, what));
        }
        self.func_lines.insert(key.clone(), f.line);
        self.func_files.insert(key.clone(), f.file.clone());
        self.funcs.insert(
            key,
            FuncInfo {
                params: f.params.clone(),
                ret: f.ret.clone(),
                mut_paths: f.mut_paths.clone(),
                file: f.file.clone(),
                public,
            },
        );
    }

    /// Сообщает, что элемент объявлен без `pub` и потому не виден из
    /// другого файла.
    fn private(&mut self, line: usize, what: &str, name: &str, file: &str) {
        self.diags.push(Diagnostic::new(
            line,
            format!(
                "{} `{}` объявлен без `pub` в `{}` — из другого файла не виден",
                what, name, file
            ),
        ));
    }

    /// Ключ функции для таблиц, где важен владелец: `Тип::имя` для метода
    /// или ассоциированной функции, просто `имя` для свободной. Без этого
    /// методы разных типов с одинаковым именем перетирали бы друг друга.
    pub fn func_key(f: &FuncDecl) -> String {
        match &f.owner {
            Some(ty) => format!("{}::{}", ty, f.name),
            None => f.name.clone(),
        }
    }

    /// Имена параметров функции — только их пути требуют mut-контракта.
    /// Имена параметров-ссылок: только они требуют mut-контракта.
    ///
    /// Параметр по значению перемещён в функцию — мутация его не видна
    /// вызывающему, поэтому объявлять её не нужно (как и локальные
    /// переменные). Контракт нужен только для ссылок: там мутация
    /// инвалидирует ссылки вызывающего.
    fn ref_param_names(f: &FuncDecl) -> Vec<String> {
        f.params
            .iter()
            .filter(|p| !matches!(p.kind, ParamKind::Owned))
            .map(|p| p.name.clone())
            .collect()
    }

    // --------------------------------------------------------
    // Проверка функции
    // --------------------------------------------------------

    fn check_func(&mut self, f: &FuncDecl) {
        self.refs.clear();
        self.modified.clear();

        let mut scope: HashMap<String, Local> = HashMap::new();

        for p in &f.params {
            let uid = self.next_uid;
            self.next_uid += 1;
            self.names.insert(uid, p.name.clone());

            let (ty, ref_target) = match &p.kind {
                ParamKind::Owned => (p.ty.clone(), None),
                // Ссылка на объект целиком: путь = корень параметра.
                ParamKind::Ref => (
                    Type::Ref(Box::new(p.ty.clone())),
                    Some(Path::root(&p.name)),
                ),
                // Ссылка внутрь: путь из сигнатуры, корень — имя
                // параметра-владельца (например `e in entities[]`).
                ParamKind::RefIn(path) => {
                    let inner = self.infer_param_type(f, path);
                    self.param_types
                        .insert((Self::func_key(f), p.name.clone()), inner.clone());
                    (Type::Ref(Box::new(inner)), Some(path.clone()))
                }
            };

            let mut local = Local {
                uid,
                ty,
                path: Path::root(&p.name),
                ref_target,
                ref_uid: None,
                moved: false,
                is_mut: p.mutable,
                escapes: Vec::new(),
            };

            // Путь параметра-ссылки должен начинаться с его же имени либо
            // с имени другого параметра (например `x in world.entities[]`).
            if let ParamKind::RefIn(path) = &p.kind {
                local.path = Path::root(&p.name);
                let _ = path;
            }

            if let Some(t) = &local.ref_target {
                local.ref_uid = Some(uid);
                self.refs.push(RefLocal {
                    uid,
                    targets: vec![t.clone()],
                    dead: false,
                    created_line: 0,
                    killed: None,
                    imprecise: false,
                });
            }

            scope.insert(p.name.clone(), local);
        }

        // Проверяем тело.
        // `declared` — пути, которые функция объявляет изменёнными.
        // Они же дают право писать: запись разрешена, только если путь
        // попадает в объявленный.
        let mut declared: Vec<Path> = f
            .mut_paths
            .iter()
            .map(|mp| Path { root: mp.param.clone(), segs: mp.segs.clone() })
            .collect();
        // `mut` на параметре-ссылке = «меняется содержимое этого параметра».
        for p in &f.params {
            if p.mutable {
                match &p.kind {
                    ParamKind::Ref => declared.push(Path::root(&p.name)),
                    ParamKind::RefIn(path) => {
                        declared.push(path.clone());
                    }
                    ParamKind::Owned => {}
                }
            }
        }

        let mut ctx = Ctx {
            scope,
            ret: f.ret.clone(),
            declared: declared.clone(),
            loop_depth: 0,
            loops: Vec::new(),
            is_main: f.name == "main",
            diverged: false,
        };

        // Контракты: `pre` проверяется на входе, `post` — перед каждым
        // `return`. Условия — обычные выражения типа bool.
        for e in &f.pre {
            let t = self.type_of(e, &mut ctx);
            if t != Type::Bool && t != Type::Void {
                self.diags.push(Diagnostic::new(
                    e.line,
                    format!("условие `pre` должно быть bool, получено `{}`", t.name()),
                ));
            }
        }

        self.check_block(&f.body, &mut ctx);

        // `post` видит конечное состояние тела и имя `result` —
        // возвращаемое значение. В `void`-функции `result` нет.
        let mut post_ctx = Ctx {
            scope: ctx.scope.clone(),
            ret: ctx.ret.clone(),
            declared: ctx.declared.clone(),
            loop_depth: 0,
            loops: Vec::new(),
            is_main: ctx.is_main,
            diverged: false,
        };
        if f.ret != Type::Void {
            let uid = self.next_uid;
            self.next_uid += 1;
            self.names.insert(uid, "result".to_string());
            post_ctx.scope.insert(
                "result".to_string(),
                Local {
                    uid,
                    ty: f.ret.clone(),
                    path: Path::root("result"),
                    ref_target: None,
                    ref_uid: None,
                    moved: false,
                    is_mut: false,
                    escapes: Vec::new(),
                },
            );
        }
        for e in &f.post {
            if f.ret == Type::Void && mentions_ident(e, "result") {
                self.diags.push(Diagnostic::new(
                    e.line,
                    "`result` доступен только в post функции, возвращающей значение"
                        .to_string(),
                ));
                continue;
            }
            let t = self.type_of(e, &mut post_ctx);
            if t != Type::Bool && t != Type::Void {
                self.diags.push(Diagnostic::new(
                    e.line,
                    format!("условие `post` должно быть bool, получено `{}`", t.name()),
                ));
            }
        }

        // Проверка mut-контракта: тело не должно менять то, что не объявлено.
        // Локальные переменные — собственность функции, их объявлять не нужно;
        // контракт касается только путей, уходящих в параметры.
        let params = Self::ref_param_names(f);
        for (path, _is_replace) in self.modified.clone() {
            // Пропускаем изменения локальных переменных.
            if !params.contains(&path.root) {
                continue;
            }
            if !declared.iter().any(|d| d.is_prefix_of(&path)) {
                self.diags.push(Diagnostic::new(
                    f.line,
                    format!(
                        "функция `{}` изменяет `{}`, но не объявляет это в mut-контракте \
                         (объявлено: {})",
                        f.name,
                        self.render(&path),
                        if declared.is_empty() {
                            "ничего".to_string()
                        } else {
                            declared
                                .iter()
                                .map(|d| self.render(d))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                ));
            }
        }

        // Автоматическое освобождение Vec: в конце функции освобождаем
        // все Vec-локальные, которые не были перемещены и не освобождены
        // явно. Это конец области видимости, а не liveness (п. 3.10):
        // liveness требует отслеживания последнего использования и
        // вставки free раньше — следующий шаг.
        let mut drops: Vec<(String, Type, Vec<String>)> = Vec::new();
        for (name, l) in &ctx.scope {
            if l.moved {
                continue;
            }
            if !matches!(l.ty, Type::Vec(_)) {
                continue;
            }
            // Параметры не освобождаем: владелец — вызывающий.
            if f.params.iter().any(|p| &p.name == name) {
                continue;
            }
            // Псевдонимы: локальные ссылки, указывающие внутрь этого Vec.
            // Освобождать раньше их последнего использования нельзя.
            let mut aliases = Vec::new();
            for (an, al) in &ctx.scope {
                if an == name {
                    continue;
                }
                let targets: Vec<Path> = if let Some(uid) = al.ref_uid {
                    self.refs
                        .iter()
                        .find(|r| r.uid == uid)
                        .map(|r| r.targets.clone())
                        .unwrap_or_default()
                } else {
                    al.ref_target.clone().into_iter().collect()
                };
                if targets.iter().any(|t| l.path.may_cover(t)) {
                    aliases.push(an.clone());
                }
            }
            drops.push((name.clone(), l.ty.clone(), aliases));
        }
        if !drops.is_empty() {
            self.vec_drops.insert(Self::func_key(f), drops);
        }
    }

    /// Тип элемента, на который указывает путь внутри сигнатуры.
    fn infer_param_type(&self, f: &FuncDecl, path: &Path) -> Type {
        // Ищем параметр-владелец.
        let owner = f.params.iter().find(|p| p.name == path.root);
        let Some(owner) = owner else { return Type::Void };
        let mut ty = match &owner.kind {
            ParamKind::Owned => owner.ty.clone(),
            ParamKind::Ref => owner.ty.deref().clone(),
            ParamKind::RefIn(_) => Type::Void,
        };
        // Проходим по сегментам.
        for seg in &path.segs {
            match seg {
                Seg::Field(name) => {
                    let base = ty.deref().clone();
                    ty = match base {
                        Type::Named(sn) => self
                            .structs
                            .get(&sn)
                            .and_then(|s| s.fields.iter().find(|f| &f.name == name))
                            .map(|f| f.ty.clone())
                            .unwrap_or(Type::Void),
                        _ => Type::Void,
                    };
                }
                Seg::Elems => {
                    ty = match ty.deref().clone() {
                        Type::Array(elem, _) => *elem,
                        _ => Type::Void,
                    };
                }
                Seg::Idx(_) => {}
            }
        }
        ty
    }

    /// Общая проверка `match` по фиксированному набору вариантов:
    /// пользовательский enum, `Option` или `Result`.
    fn check_match_variants(
        &mut self,
        line: usize,
        arms: &[MatchArm],
        en: &str,
        variants: &[(String, Vec<Type>)],
        ctx: &mut Ctx,
    ) {
        let base_scope = ctx.scope.clone();
        let base_refs = self.refs.clone();
        let base_diverged = ctx.diverged;
        let mut merged_scope = base_scope.clone();
        let mut merged_refs = base_refs.clone();
        let mut all_diverged = !arms.is_empty();
        let mut seen: Vec<String> = Vec::new();
        for arm in arms {
            let variant = variants.iter().find(|(n, _)| n == &arm.variant).cloned();
            if arm.ty != en {
                self.diags.push(Diagnostic::new(
                    arm.line,
                    format!(
                        "ветвь `{}::{}` не относится к enum `{}`",
                        arm.ty, arm.variant, en
                    ),
                ));
            } else if variant.is_none() {
                self.diags.push(Diagnostic::new(
                    arm.line,
                    format!("у enum `{}` нет варианта `{}`", en, arm.variant),
                ));
            } else if seen.contains(&arm.variant) {
                self.diags.push(Diagnostic::new(
                    arm.line,
                    format!("вариант `{}` разобран дважды", arm.variant),
                ));
            } else {
                seen.push(arm.variant.clone());
            }

            // Каждая ветвь стартует с одного состояния.
            ctx.scope = base_scope.clone();
            self.refs = base_refs.clone();
            ctx.diverged = false;

            // Привязки данных варианта: `Some(x)`.
            let mut saved: Vec<(String, Option<Local>)> = Vec::new();
            if let Some((_, payload)) = &variant {
                if payload.len() != arm.bindings.len() {
                    self.diags.push(Diagnostic::new(
                        arm.line,
                        format!(
                            "вариант `{}::{}` несёт {}, в ветви привязано {}",
                            en,
                            arm.variant,
                            аргументов(payload.len()),
                            arm.bindings.len()
                        ),
                    ));
                }
                for (b, pt) in arm.bindings.iter().zip(payload.iter()) {
                    saved.push((b.clone(), ctx.scope.get(b).cloned()));
                    let uid = self.next_uid;
                    self.next_uid += 1;
                    self.names.insert(uid, b.clone());
                    ctx.scope.insert(
                        b.clone(),
                        Local {
                            uid,
                            ty: pt.clone(),
                            path: Path::root(&uid.to_string()),
                            ref_target: None,
                            ref_uid: None,
                            moved: false,
                            is_mut: false,
                            escapes: Vec::new(),
                        },
                    );
                }
            }
            let diverged = self.check_block_scoped(&arm.body, ctx);
            for (b, prev) in saved {
                match prev {
                    Some(l) => {
                        ctx.scope.insert(b, l);
                    }
                    None => {
                        ctx.scope.remove(&b);
                    }
                }
            }
            // Сливаем состояние ветви: перемещено/утечка, если так
            // в любой ветви. Ветвь с `return`/`break`/`continue` не
            // вливается — её состояние уходит не сюда.
            if !diverged {
                for (name, l) in &ctx.scope {
                    if let Some(m) = merged_scope.get_mut(name).filter(|x| x.uid == l.uid) {
                        m.moved = m.moved || l.moved;
                        for e in &l.escapes {
                            if !m.escapes.contains(e) {
                                m.escapes.push(e.clone());
                            }
                        }
                    }
                }
                for r in &self.refs {
                    if r.dead {
                        if let Some(m) = merged_refs.iter_mut().find(|x| x.uid == r.uid) {
                            m.dead = true;
                            m.killed = r.killed.clone();
                        }
                    }
                }
            }
            all_diverged &= diverged;
        }
        ctx.scope = merged_scope;
        self.refs = merged_refs;
        ctx.diverged = base_diverged || all_diverged;
        let missing: Vec<String> = variants
            .iter()
            .filter(|(n, _)| !seen.contains(n))
            .map(|(n, _)| n.clone())
            .collect();
        if !missing.is_empty() {
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "`match` не исчерпывающий: не разобраны варианты {}",
                    missing
                        .iter()
                        .map(|v| format!("`{}::{}`", en, v))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    /// Методы встроенного `Vec<T>`: `push`, `len`, `get`, `pop`, `free`.
    fn type_of_vec_method(
        &mut self,
        e: &Expr,
        obj: &Expr,
        name: &str,
        args: &[Expr],
        elem: &Type,
        ctx: &mut Ctx,
    ) -> Type {
        match name {
            "push" => {
                if args.len() != 1 {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`push` принимает один аргумент, получено {}", args.len()),
                    ));
                }
                let (path, root_kind) = self.resolve_place(obj, ctx);
                self.check_mut_path(&path, &root_kind, ctx, e.line, "push");
                if let Some(a) = args.first() {
                    let at = self.type_of_expected(a, Some(elem), ctx);
                    if !self.types_compatible(elem.clone(), &at) {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!("`push` ждёт `{}`, получен `{}`", elem.name(), at.name()),
                        ));
                    }
                    self.record_move(a, &at, ctx);
                }
                // Перевыделение буфера гасит ссылки на элементы.
                self.invalidate_inside(&path, e.line, "push");
                Type::Void
            }
            "len" => {
                if !args.is_empty() {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`len` не принимает аргументов, получено {}", args.len()),
                    ));
                }
                Type::Int
            }
            "get" => {
                if args.len() != 1 {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`get` принимает один аргумент, получено {}", args.len()),
                    ));
                }
                if let Some(a) = args.first() {
                    let at = self.type_of(a, ctx);
                    if at.deref() != &Type::Int {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!("`get` ждёт индекс типа `int`, получен `{}`", at.name()),
                        ));
                    }
                }
                let ty = Type::Option(Box::new(elem.clone()));
                self.record_option_inst(&ty);
                ty
            }
            "pop" => {
                if !args.is_empty() {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`pop` не принимает аргументов, получено {}", args.len()),
                    ));
                }
                let (path, root_kind) = self.resolve_place(obj, ctx);
                self.check_mut_path(&path, &root_kind, ctx, e.line, "pop");
                self.invalidate_inside(&path, e.line, "pop");
                let ty = Type::Option(Box::new(elem.clone()));
                self.record_option_inst(&ty);
                ty
            }
            "free" => {
                if !args.is_empty() {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`free` не принимает аргументов, получено {}", args.len()),
                    ));
                }
                let (path, root_kind) = self.resolve_place(obj, ctx);
                self.check_mut_path(&path, &root_kind, ctx, e.line, "free");
                self.invalidate_inside(&path, e.line, "free");
                // После `free` значение больше не используется: помечаем
                // локальную перемещённой, чтобы автоматическое освобождение
                // в конце функции не сделало double-free.
                if let RootKind::Owned(uid) = root_kind {
                    if let Some(l) = ctx.scope.values_mut().find(|l| l.uid == uid) {
                        l.moved = true;
                    }
                }
                Type::Void
            }
            _ => {
                self.diags.push(Diagnostic::new(
                    e.line,
                    format!("у `Vec` нет метода `{}`", name),
                ));
                Type::Void
            }
        }
    }

    /// Получатель `push`/`pop`/`free` должен быть изменяемым.
    fn check_mut_path(
        &mut self,
        path: &Path,
        root_kind: &RootKind,
        ctx: &Ctx,
        line: usize,
        method: &str,
    ) {
        let allowed = match root_kind {
            RootKind::Owned(uid) => ctx
                .scope
                .values()
                .find(|l| l.uid == *uid)
                .map(|l| l.is_mut)
                .unwrap_or(false),
            RootKind::ThroughRef => {
                let target = path.clone();
                let root_uid = target.root.parse::<u32>().ok();
                match root_uid {
                    Some(_) => {
                        let owner = ctx
                            .scope
                            .values()
                            .filter(|l| !l.ref_target.is_some())
                            .filter(|l| l.path.is_prefix_of(&target))
                            .max_by_key(|l| (l.path.segs.len(), l.uid));
                        owner.map(|l| l.is_mut).unwrap_or(false)
                    }
                    None => ctx
                        .declared
                        .iter()
                        .any(|d| d.is_prefix_of(&target) || target.is_prefix_of(d)),
                }
            }
        };
        if !allowed {
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "`{}` меняет `{}`: получатель должен быть `mut`",
                    method,
                    self.render(path)
                ),
            ));
        }
    }

    fn check_block(&mut self, b: &Block, ctx: &mut Ctx) {
        for (i, stmt) in b.stmts.iter().enumerate() {
            if ctx.diverged {
                self.check_unreachable_rest(&b.stmts[i..], ctx);
                break;
            }
            self.check_stmt(stmt, ctx);
        }
    }

    /// Код после `return`/`break`/`continue` недостижим, но его типы
    /// нужны кодогенератору: иначе он подставит значения по умолчанию
    /// и C не соберётся. Проверяем весь хвост с общим состоянием, без
    /// диагностик, и откатываем состояние.
    fn check_unreachable_rest(&mut self, stmts: &[Stmt], ctx: &mut Ctx) {
        let saved_diags = self.diags.items.len();
        let saved_scope = ctx.scope.clone();
        let saved_refs = self.refs.clone();
        let saved_diverged = ctx.diverged;
        let saved_loops = ctx.loops.clone();
        let saved_modified = self.modified.clone();
        for stmt in stmts {
            self.check_stmt(stmt, ctx);
        }
        self.diags.items.truncate(saved_diags);
        ctx.scope = saved_scope;
        self.refs = saved_refs;
        ctx.diverged = saved_diverged;
        ctx.loops = saved_loops;
        self.modified = saved_modified;
    }

    /// Блок как отдельная область видимости: локальные переменные
    /// исчезают после него, состояние внешних сохраняется.
    /// Возвращает, расходится ли блок (`return`/`break`/`continue`).
    fn check_block_scoped(&mut self, b: &Block, ctx: &mut Ctx) -> bool {
        let saved_scope = ctx.scope.clone();
        let saved_diverged = ctx.diverged;
        ctx.diverged = false;
        for (i, stmt) in b.stmts.iter().enumerate() {
            if ctx.diverged {
                self.check_unreachable_rest(&b.stmts[i..], ctx);
                break;
            }
            self.check_stmt(stmt, ctx);
        }
        let diverged = ctx.diverged;
        // Убираем локальные переменные блока, сохраняя состояние внешних.
        let mut result = saved_scope;
        for (name, l) in &ctx.scope {
            if let Some(s) = result.get(name) {
                if s.uid == l.uid {
                    result.insert(name.clone(), l.clone());
                }
            }
        }
        ctx.scope = result;
        self.refs.retain(|r| ctx.scope.values().any(|l| l.uid == r.uid));
        ctx.diverged = saved_diverged || diverged;
        diverged
    }

    fn check_closure_body(&mut self, body: &Block, ctx: &mut Ctx) -> Type {
        let mut ret = Type::Void;
        for stmt in &body.stmts {
            self.check_stmt(stmt, ctx);
            if let Stmt::Expr(e) = stmt {
                ret = self.type_of(e, ctx);
            }
        }
        ret
    }

    fn check_stmt(&mut self, stmt: &Stmt, ctx: &mut Ctx) {
        match stmt {
            Stmt::Let { name, mutable, ty, init } => {
                let mut init_ty = self.type_of_expected(init, ty.as_ref(), ctx);
                // Пустой литерал `[]` не позволяет вывести тип элемента:
                // элементов нет, а значит нет и их типа. Если тип
                // объявлен явно, пустой литерал наследует его.
                if let Some(declared) = ty {
                    if let ExprKind::Array(items) = &init.kind {
                        if items.is_empty() {
                            if let Type::Array(..) = declared {
                                init_ty = declared.clone();
                            }
                        }
                    }
                }
                // Пустой литерал без объявленного типа: элементов нет,
                // поэтому тип элемента вывести не из чего. Сообщаем об
                // этом явно — иначе ошибка всплыла бы только при
                // компиляции C, в виде «array of voids».
                if let ExprKind::Array(items) = &init.kind {
                    if items.is_empty() && ty.is_none() {
                        self.diags.push(Diagnostic::new(
                            init.line,
                            "невозможно вывести тип элемента пустого массива: \
                             укажите тип явно, например `let xs [int; 0] = [];`"
                                .to_string(),
                        ));
                    }
                }
                if let Some(declared) = ty {
                    if let Type::Slice(inner) = declared {
                        self.record_slice_inst(inner);
                    }
                    if !self.types_compatible(declared.clone(), &init_ty) {
                        self.diags.push(Diagnostic::new(
                            init.line,
                            format!(
                                "несовпадение типов: объявлено `{}`, получено `{}`",
                                declared.name(),
                                init_ty.name()
                            ),
                        ));
                    }
                }
                // Перемещение: только для владения, не для ссылок.
                // Срез — это заимствование массива, а не перемещение.
                let is_slice = matches!(ty, Some(Type::Slice(_)));
                if !init_ty.is_ref() && !is_slice {
                    self.record_move(init, &init_ty, ctx);
                }
                let uid = self.next_uid;
                self.next_uid += 1;
                self.names.insert(uid, name.clone());

                // Если инициализатор — ссылка или срез, запоминаем её
                // цель. Результат вызова и `if` не дают точного пути:
                // берём все ref-аргументы (ветви) и помечаем цель неточной.
                let is_slice = matches!(ty, Some(Type::Slice(_))) || matches!(init_ty, Type::Slice(_));
                let (ref_targets, imprecise) = if init_ty.is_ref() || is_slice {
                    if let Some(p) = self.place_path_of(init, ctx) {
                        // Срез — вид на массив: путь — сам массив, поэтому
                        // `s[0] = 9` даёт `arr[0]` и гасит ссылку на элемент.
                        (vec![p], false)
                    } else if let Some(ps) = self.call_ref_target(init, ctx) {
                        (ps, true)
                    } else if let Some(ps) = self.if_ref_target(init, ctx) {
                        (ps, true)
                    } else {
                        (Vec::new(), false)
                    }
                } else {
                    (Vec::new(), false)
                };
                let ref_uid = if ref_targets.is_empty() {
                    None
                } else {
                    self.refs.push(RefLocal {
                        uid,
                        targets: ref_targets.clone(),
                        dead: false,
                        created_line: init.line,
                        killed: None,
                        imprecise,
                    });
                    Some(uid)
                };

                // Ссылка на локальную переменную: её путь — путь самой
                // переменной, а не новый корень. Иначе инвалидация
                // не сработала бы.
                let own_path = if init_ty.is_ref() || is_slice {
                    ref_targets.first().cloned().unwrap_or_else(|| Path::root(&uid.to_string()))
                } else {
                    Path::root(&uid.to_string())
                };

                // Ссылки на локальные, спрятанные внутри значения:
                // `let h = H(&i);` — сам `h` не ссылка, но внутри
                // лежит ссылка на локальную `i`.
                let escapes = self.escapes_in(init, ctx);

                ctx.scope.insert(
                    name.clone(),
                    Local {
                        uid,
                        ty: ty.clone().unwrap_or(init_ty),
                        path: own_path,
                        ref_target: ref_targets.first().cloned(),
                        ref_uid,
                        moved: false,
                        is_mut: *mutable,
                        escapes,
                    },
                );
            }
            Stmt::Assign { lhs, rhs } => {
                self.check_assign(lhs, rhs, ctx);
            }
            Stmt::Expr(e) => {
                self.type_of(e, ctx);
            }
            Stmt::Return { line, value } => {
                ctx.diverged = true;
                match value {
                    Some(e) => {
                        let ret = ctx.ret.clone();
                        let t = self.type_of_expected(e, Some(&ret), ctx);
                        if !self.types_compatible(ctx.ret.clone(), &t) {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!(
                                    "тип возврата: ожидался `{}`, получен `{}`",
                                    ctx.ret.name(),
                                    t.name()
                                ),
                            ));
                        }
                        // Ссылка не должна пережить кадр, в котором создана.
                        self.check_return_escapes(e, ctx);
                        self.record_move(e, &t, ctx);
                    }
                    // `return;` без значения — только из функции, которая
                    // ничего не возвращает. Иначе в C получилось бы
                    // `return;` внутри функции, возвращающей значение:
                    // gcc на это ругается, а сообщение читалось бы как
                    // ошибка бэкенда, а не как ошибка программы.
                    None => {
                        // В main голый `return;` — это «выход с кодом 0»:
                        // main особый, как в C. В остальных функциях
                        // `return;` без значения требует, чтобы функция
                        // ничего не возвращала.
                        if ctx.ret != Type::Void && !ctx.is_main {
                            self.diags.push(Diagnostic::new(
                                *line,
                                format!(
                                    "функция возвращает `{}`, поэтому `return` требует значение",
                                    ctx.ret.name()
                                ),
                            ));
                        }
                    }
                }
            }
            Stmt::Match { line, subject, arms } => {
                let t = self.type_of(subject, ctx).deref().clone();
                if let Type::Option(inner) = &t {
                    let variants = vec![
                        ("Some".to_string(), vec![(**inner).clone()]),
                        ("None".to_string(), Vec::new()),
                    ];
                    self.check_match_variants(*line, arms, "Option", &variants, ctx);
                    return;
                }
                if let Type::Result(ok, err) = &t {
                    let variants = vec![
                        ("Ok".to_string(), vec![(**ok).clone()]),
                        ("Err".to_string(), vec![(**err).clone()]),
                    ];
                    self.check_match_variants(*line, arms, "Result", &variants, ctx);
                    return;
                }
                let enum_name = match &t {
                    Type::Named(n) if self.enums.contains_key(n) => Some(n.clone()),
                    _ => None,
                };
                let Some(en) = enum_name else {
                    self.diags.push(Diagnostic::new(
                        *line,
                        format!("`match` требует enum, получено `{}`", t.name()),
                    ));
                    for arm in arms {
                        self.check_block(&arm.body, ctx);
                    }
                    return;
                };
                let decl = self.enums.get(&en).cloned().unwrap();
                let variants: Vec<(String, Vec<Type>)> = decl
                    .variants
                    .iter()
                    .map(|v| (v.name.clone(), v.payload.clone()))
                    .collect();
                self.check_match_variants(*line, arms, &en, &variants, ctx);
            }
            Stmt::If { cond, then_block, else_block } => {
                let ct = self.type_of(cond, ctx);
                if ct != Type::Bool && ct != Type::Void {
                    self.diags.push(Diagnostic::new(
                        cond.line,
                        format!("условие `if` должно быть bool, получено `{}`", ct.name()),
                    ));
                }
                let base_scope = ctx.scope.clone();
                let base_refs = self.refs.clone();
                let base_diverged = ctx.diverged;

                ctx.diverged = false;
                let then_diverged = self.check_block_scoped(then_block, ctx);
                let then_scope = ctx.scope.clone();
                let then_refs = self.refs.clone();

                // else стартует с состояния ДО then: иначе move в then
                // даёт ложное «уже перемещён» в else.
                ctx.scope = base_scope.clone();
                self.refs = base_refs.clone();
                ctx.diverged = false;
                let else_diverged = if let Some(eb) = else_block {
                    self.check_block_scoped(eb, ctx)
                } else {
                    false
                };
                let else_scope = ctx.scope.clone();
                let else_refs = self.refs.clone();

                let (scope, refs, diverged) = merge_if(
                    &base_scope,
                    &base_refs,
                    &then_scope,
                    &then_refs,
                    then_diverged,
                    &else_scope,
                    &else_refs,
                    else_diverged,
                );
                ctx.scope = scope;
                self.refs = refs;
                ctx.diverged = base_diverged || diverged;
            }
            Stmt::While { cond, body } => {
                let ct = self.type_of(cond, ctx);
                if ct != Type::Bool && ct != Type::Void {
                    self.diags.push(Diagnostic::new(
                        cond.line,
                        format!("условие `while` должно быть bool, получено `{}`", ct.name()),
                    ));
                }
                // Тело проверяем дважды: второй проход стартует с состояния
                // после первого, поэтому ловит использование ссылки,
                // инвалидированной в предыдущей итерации. Условие
                // перепроверяем после тела — оно выполняется каждую итерацию.
                ctx.loops.push(LoopState::default());
                ctx.loop_depth += 1;
                self.check_block_scoped(body, ctx);
                let cont = ctx.loops.last().map(|l| l.continues.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &cont);
                self.type_of(cond, ctx);
                self.check_block_scoped(body, ctx);
                let brk = ctx.loops.last().map(|l| l.breaks.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &brk);
                ctx.loops.pop();
                ctx.loop_depth -= 1;
                ctx.diverged = false;
            }
            Stmt::DoWhile { body, cond } => {
                // Условие выполняется ПОСЛЕ тела: ссылка, убитая в теле,
                // должна ловиться в условии.
                ctx.loops.push(LoopState::default());
                ctx.loop_depth += 1;
                self.check_block_scoped(body, ctx);
                let cont = ctx.loops.last().map(|l| l.continues.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &cont);
                let ct = self.type_of(cond, ctx);
                if ct != Type::Bool && ct != Type::Void {
                    self.diags.push(Diagnostic::new(
                        cond.line,
                        format!("условие `do-while` должно быть bool, получено `{}`", ct.name()),
                    ));
                }
                self.check_block_scoped(body, ctx);
                let brk = ctx.loops.last().map(|l| l.breaks.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &brk);
                ctx.loops.pop();
                ctx.loop_depth -= 1;
                ctx.diverged = false;
            }
            Stmt::Break { line } => {
                if ctx.loop_depth == 0 {
                    self.diags.push(Diagnostic::new(
                        *line,
                        "`break` используется вне цикла".to_string(),
                    ));
                } else {
                    let state = (ctx.scope.clone(), self.refs.clone());
                    if let Some(l) = ctx.loops.last_mut() {
                        l.breaks.push(state);
                    }
                }
                ctx.diverged = true;
            }
            Stmt::Continue { line } => {
                if ctx.loop_depth == 0 {
                    self.diags.push(Diagnostic::new(
                        *line,
                        "`continue` используется вне цикла".to_string(),
                    ));
                } else {
                    let state = (ctx.scope.clone(), self.refs.clone());
                    if let Some(l) = ctx.loops.last_mut() {
                        l.continues.push(state);
                    }
                }
                ctx.diverged = true;
            }
            Stmt::ForRange { line, var, start, end, body } => {
                let st = self.type_of(start, ctx);
                let et = self.type_of(end, ctx);
                for (what, t) in [("начало", &st), ("конец", &et)] {
                    if *t != Type::Int {
                        self.diags.push(Diagnostic::new(
                            *line,
                            format!(
                                "граница диапазона ({}) должна быть int, получено `{}`",
                                what,
                                t.name()
                            ),
                        ));
                    }
                }
                // Переменная цикла — обычная локальная переменная.
                self.declare_loop_var(var, Type::Int, ctx);
                ctx.loops.push(LoopState::default());
                ctx.loop_depth += 1;
                self.check_block_scoped(body, ctx);
                let cont = ctx.loops.last().map(|l| l.continues.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &cont);
                // Как и у `while`, тело проверяем дважды.
                self.check_block_scoped(body, ctx);
                let brk = ctx.loops.last().map(|l| l.breaks.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &brk);
                ctx.loops.pop();
                ctx.loop_depth -= 1;
                ctx.diverged = false;
            }
            Stmt::ForEach { line, var, iter, body } => {
                let it = self.type_of(iter, ctx);
                let it = it.deref().clone();
                let elem_ty = match &it {
                    Type::Array(elem, _) => (**elem).clone(),
                    other => {
                        self.diags.push(Diagnostic::new(
                            *line,
                            format!(
                                "`for ... in` работает по массиву, получено `{}`",
                                other.name()
                            ),
                        ));
                        Type::Int
                    }
                };
                // Обход даёт копию элемента (см. lang-design), поэтому
                // массив остаётся доступен после цикла.
                self.declare_loop_var(var, elem_ty, ctx);
                ctx.loops.push(LoopState::default());
                ctx.loop_depth += 1;
                self.check_block_scoped(body, ctx);
                let cont = ctx.loops.last().map(|l| l.continues.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &cont);
                self.check_block_scoped(body, ctx);
                let brk = ctx.loops.last().map(|l| l.breaks.clone()).unwrap_or_default();
                merge_loop_states(&mut ctx.scope, &mut self.refs, &brk);
                ctx.loops.pop();
                ctx.loop_depth -= 1;
                ctx.diverged = false;
            }
        }
    }

    /// Объявляет переменную цикла как обычную локальную переменную.
    fn declare_loop_var(&mut self, name: &str, ty: Type, ctx: &mut Ctx) {
        let uid = self.next_uid;
        self.next_uid += 1;
        self.names.insert(uid, name.to_string());
        ctx.scope.insert(
            name.to_string(),
            Local {
                uid,
                ty,
                path: Path::root(&uid.to_string()),
                ref_target: None,
                ref_uid: None,
                moved: false,
                is_mut: false,
                escapes: Vec::new(),
            },
        );
    }

    fn check_assign(&mut self, lhs: &Expr, rhs: &Expr, ctx: &mut Ctx) {
        // LHS — место, а не использование: `a = ...` переинициализирует
        // перемещённую `a`, поэтому проверка «уже перемещён» здесь не нужна.
        let lhs_moved = if let ExprKind::Ident(name) = &lhs.kind {
            ctx.scope.get_mut(name).map(|l| {
                let m = l.moved;
                l.moved = false;
                m
            })
        } else {
            None
        };
        let (path, root_kind) = self.resolve_place(lhs, ctx);
        let lhs_ty = self.type_of(lhs, ctx);
        // Восстанавливаем: rhs должен видеть исходное состояние.
        if let (ExprKind::Ident(name), Some(m)) = (&lhs.kind, lhs_moved) {
            if let Some(l) = ctx.scope.get_mut(name) {
                l.moved = m;
            }
        }
        let rhs_ty = self.type_of(rhs, ctx);

        if !self.types_compatible(lhs_ty.clone(), &rhs_ty) {
            self.diags.push(Diagnostic::new(
                rhs.line,
                format!(
                    "несовпадение типов при присваивании: `{}` и `{}`",
                    lhs_ty.name(),
                    rhs_ty.name()
                ),
            ));
        }

        // Изменяемость: владелец должен быть `mut`, либо писать идёт через
        // ссылку на объявленный изменяемый путь.
        match &root_kind {
            RootKind::Owned(uid) => {
                let name = self.names.get(uid).cloned().unwrap_or_default();
                let is_mut = ctx
                    .scope
                    .values()
                    .find(|l| l.uid == *uid)
                    .map(|l| l.is_mut)
                    .unwrap_or(false);
                if !is_mut {
                    self.diags.push(Diagnostic::new(
                        lhs.line,
                        format!("`{}` не объявлен как `mut` — присваивание запрещено", name),
                    ));
                }
            }
            RootKind::ThroughRef => {
                // Запись через ссылку: право определяется владельцем пути.
                // Если корень — локальная переменная, она должна быть `mut`.
                // Если корень — параметр, путь должен быть объявлен
                // изменяемым в mut-контракте.
                let target = path.clone();
                let root_uid = target.root.parse::<u32>().ok();
                let allowed = match root_uid {
                    Some(_uid) => {
                        // Владелец объекта — переменная, чей путь является
                        // префиксом пути записи. Для ссылки на элемент это
                        // контейнер, а не сама ссылка.
                        let owner = ctx
                            .scope
                            .values()
                            .filter(|l| !l.ref_target.is_some())
                            .filter(|l| {
                                // Путь переменной — префикс пути записи.
                                l.path.is_prefix_of(&target)
                            })
                            // При равенстве длин пути решаем по uid —
                            // результат не должен зависеть от порядка обхода.
                            .max_by_key(|l| (l.path.segs.len(), l.uid));
                        owner.map(|l| l.is_mut).unwrap_or(false)
                    }
                    None => ctx.declared.iter().any(|d| {
                        d.is_prefix_of(&target) || target.is_prefix_of(d)
                    }),
                };
                if !allowed {
                    self.diags.push(Diagnostic::new(
                        lhs.line,
                        format!(
                            "нельзя писать в `{}`: путь не объявлен изменяемым \
                             (объявлено: {})",
                            self.render(&target),
                            if ctx.declared.is_empty() {
                                "ничего".to_string()
                            } else {
                                ctx.declared
                                    .iter()
                                    .map(|d| self.render(d))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        ),
                    ));
                }
            }
        }

        self.record_move(rhs, &rhs_ty, ctx);

        // Присваивание снова инициализирует переменную: `x = f(x)`
        // после `consume(x)` законно, а старые утечки `x` больше не
        // действуют.
        if let ExprKind::Ident(name) = &lhs.kind {
            if let Some(l) = ctx.scope.get_mut(name) {
                l.moved = false;
                l.escapes.clear();
            }
        }

        // Ссылка на локальную переменную не должна уехать из функции
        // через присваивание: `h.r = &i;` в поле ПАРАМЕТРА кладёт
        // ссылку в кадр вызывающего, который переживёт наш кадр.
        if root_kind_is_param(&path) {
            for (segs, target) in self.escapes_in(rhs, ctx) {
                let what = if segs.is_empty() {
                    format!("`{}`", self.render(&path))
                } else {
                    format!("поле `{}` в `{}`", self.render_segs(&segs), self.render(&path))
                };
                self.diags.push(Diagnostic::new(
                    rhs.line,
                    format!(
                        "{} получает ссылку на локальную переменную `{}` — \
                         она перестанет существовать после возврата; \
                         ссылаться можно только на параметр",
                        what,
                        self.render(&target)
                    ),
                ));
            }
        }

        // Присваивание ЗАМЕНЯЕТ значение по пути.
        self.modified.push((path.clone(), true));
        self.invalidate_replace(&path, lhs.line, "присваивание");

        // Присваивание в поле локальной структуры обновляет её утечки:
        // `h.r = &i` кладёт ссылку на локальную в поле `r`, и `return h`
        // должен её увидеть.
        if let Ok(uid) = path.root.parse::<u32>() {
            let local_name = ctx
                .scope
                .iter()
                .find(|(_, l)| l.uid == uid)
                .map(|(n, _)| n.clone());
            if let Some(name) = local_name {
                let local_path = ctx.scope[&name].path.clone();
                let rel_segs: Vec<Seg> = path.segs[local_path.segs.len()..].to_vec();
                let new_escapes: Vec<Escape> = self
                    .escapes_in(rhs, ctx)
                    .into_iter()
                    .map(|(mut segs, t)| {
                        let mut full = rel_segs.clone();
                        full.append(&mut segs);
                        (full, t)
                    })
                    .collect();
                if let Some(l) = ctx.scope.get_mut(&name) {
                    l.escapes.retain(|(segs, _)| !segs.starts_with(&rel_segs));
                    l.escapes.extend(new_escapes);
                }
            }
        }
    }

    // --------------------------------------------------------
    // Пути
    // --------------------------------------------------------

    /// Разрешает выражение-«место» (lvalue) в путь значения.
    fn resolve_place(&mut self, e: &Expr, ctx: &mut Ctx) -> (Path, RootKind) {
        match &e.kind {
            ExprKind::Ident(name) => {
                let local = ctx.scope.get(name).cloned();
                let Some(local) = local else {
                    return (Path::root("?"), RootKind::Owned(0));
                };
                if local.moved {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`{}` уже перемещён, использовать нельзя", name),
                    ));
                }
                match local.ref_target {
                    Some(t) => (t, RootKind::ThroughRef),
                    None => {
                        (local.path, RootKind::Owned(local.uid))
                    }
                }
            }
            ExprKind::Field { obj, name } => {
                let (p, k) = self.resolve_place(obj, ctx);
                (p.child(Seg::Field(name.clone())), k)
            }
            ExprKind::Index { obj, idx } => {
                let (p, k) = self.resolve_place(obj, ctx);
                // Конкретный индекс — свой путь; неконкретный — общая группа.
                match &idx.kind {
                    ExprKind::Int(n) => (p.child(Seg::Idx(*n)), k),
                    _ => (p.child(Seg::Elems), k),
                }
            }
            _ => (Path::root("?"), RootKind::Owned(0)),
        }
    }

    /// Инвалидирует ссылки на путь и на всё внутри него (замена значения).
    fn invalidate_replace(&mut self, path: &Path, line: usize, reason: &str) {
        for r in self.refs.iter_mut() {
            // Ссылка на сам путь и на всё внутри него — объект по этому
            // пути заменён. Ссылка на элемент с НЕИЗВЕСТНЫМ индексом
            // хранит путь-группу (`a[]`): запись в конкретный элемент
            // могла затронуть именно тот элемент, на который она
            // указывает, — значит, ссылка гаснет.
            // Неточная цель (результат вызова/`if`): гасим при любом
            // изменении внутри неё — ссылка могла указывать куда угодно.
            let hit = r.targets.iter().any(|t| {
                path.may_cover(t) || (r.imprecise && t.is_prefix_of(path))
            });
            if hit && !r.dead {
                r.dead = true;
                r.killed = Some((line, reason.to_string()));
            }
        }
    }

    /// Инвалидирует ссылки строго внутри пути (содержимое изменено).
    fn invalidate_inside(&mut self, path: &Path, line: usize, reason: &str) {
        for r in self.refs.iter_mut() {
            let hit = r.targets.iter().any(|t| {
                (path.is_prefix_of(t) && t != path)
                    || path.overlaps(t)
                    || (r.imprecise && t.is_prefix_of(path))
            });
            if hit && !r.dead {
                r.dead = true;
                r.killed = Some((line, reason.to_string()));
            }
        }
    }

    // --------------------------------------------------------
    // Выражения
    // --------------------------------------------------------

    fn type_of(&mut self, e: &Expr, ctx: &mut Ctx) -> Type {
        let t = self.type_of_inner(e, ctx);
        self.expr_types.insert(e.id, t.clone());
        t
    }

    /// Тип выражения с учётом ожидаемого типа. Нужен конструкторам
    /// `None`/`Ok`/`Err`, у которых часть параметров не выводится из
    /// аргументов: `let o Option<int> = None;`.
    fn type_of_expected(&mut self, e: &Expr, expected: Option<&Type>, ctx: &mut Ctx) -> Type {
        let t = self.type_of_expected_inner(e, expected, ctx);
        self.expr_types.insert(e.id, t.clone());
        t
    }

    fn type_of_expected_inner(
        &mut self,
        e: &Expr,
        expected: Option<&Type>,
        ctx: &mut Ctx,
    ) -> Type {
        match &e.kind {
            ExprKind::Ident(name) if name == "None" => {
                let inner = match expected {
                    Some(Type::Option(t)) => (**t).clone(),
                    _ => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            "не удалось вывести тип `None`: укажите тип, \
                             например `let o Option<int> = None;`"
                                .to_string(),
                        ));
                        Type::Int
                    }
                };
                self.check_payload_type(e.line, &inner, "Option");
                let ty = Type::Option(Box::new(inner));
                self.record_option_inst(&ty);
                ty
            }
            ExprKind::Call { callee, args } => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return self.type_of(e, ctx);
                };
                if name == "Vec::new" {
                    if !args.is_empty() {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("`Vec::new` не принимает аргументов, получено {}", args.len()),
                        ));
                    }
                    let elem = match expected {
                        Some(Type::Vec(t)) => (**t).clone(),
                        _ => {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                "не удалось вывести тип `Vec::new`: укажите тип, \
                                 например `let v Vec<int> = Vec::new();`"
                                    .to_string(),
                            ));
                            Type::Int
                        }
                    };
                    let ty = Type::Vec(Box::new(elem));
                    self.record_vec_inst(&ty);
                    return ty;
                }
                match name.as_str() {
                    "Some" => {
                        if args.len() != 1 {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("`Some` принимает один аргумент, получено {}", args.len()),
                            ));
                        }
                        let inner_expected = match expected {
                            Some(Type::Option(t)) => Some((**t).clone()),
                            _ => None,
                        };
                        let at = match args.first() {
                            Some(a) => self.type_of_expected(a, inner_expected.as_ref(), ctx),
                            None => Type::Int,
                        };
                        self.check_payload_type(e.line, &at, "Option");
                        let ty = Type::Option(Box::new(at));
                        self.record_option_inst(&ty);
                        ty
                    }
                    "Ok" => {
                        if args.len() != 1 {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("`Ok` принимает один аргумент, получено {}", args.len()),
                            ));
                        }
                        let (t_expected, e_expected) = match expected {
                            Some(Type::Result(t, e)) => {
                                (Some((**t).clone()), Some((**e).clone()))
                            }
                            _ => (None, None),
                        };
                        let at = match args.first() {
                            Some(a) => self.type_of_expected(a, t_expected.as_ref(), ctx),
                            None => Type::Int,
                        };
                        self.check_payload_type(e.line, &at, "Result");
                        let et = match e_expected {
                            Some(t) => t,
                            None => {
                                self.diags.push(Diagnostic::new(
                                    e.line,
                                    "не удалось вывести тип ошибки `Ok`: укажите тип, \
                                     например `let r Result<int, string> = Ok(5);`"
                                        .to_string(),
                                ));
                                Type::Str
                            }
                        };
                        let ty = Type::Result(Box::new(at), Box::new(et));
                        self.record_result_inst(&ty);
                        ty
                    }
                    "Err" => {
                        if args.len() != 1 {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("`Err` принимает один аргумент, получено {}", args.len()),
                            ));
                        }
                        let (t_expected, e_expected) = match expected {
                            Some(Type::Result(t, e)) => {
                                (Some((**t).clone()), Some((**e).clone()))
                            }
                            _ => (None, None),
                        };
                        let et = match args.first() {
                            Some(a) => self.type_of_expected(a, e_expected.as_ref(), ctx),
                            None => Type::Str,
                        };
                        self.check_payload_type(e.line, &et, "Result");
                        let tt = match t_expected {
                            Some(t) => t,
                            None => {
                                self.diags.push(Diagnostic::new(
                                    e.line,
                                    "не удалось вывести тип значения `Err`: укажите тип, \
                                     например `let r Result<int, string> = Err(\"bad\");`"
                                        .to_string(),
                                ));
                                Type::Int
                            }
                        };
                        let ty = Type::Result(Box::new(tt), Box::new(et));
                        self.record_result_inst(&ty);
                        ty
                    }
                    _ => self.type_of(e, ctx),
                }
            }
            _ => self.type_of(e, ctx),
        }
    }

    fn record_option_inst(&mut self, ty: &Type) {
        if !self.option_insts.contains(ty) {
            self.option_insts.push(ty.clone());
        }
    }

    fn record_result_inst(&mut self, ty: &Type) {
        if let Type::Result(t, e) = ty {
            let pair = ((**t).clone(), (**e).clone());
            if !self.result_insts.contains(&pair) {
                self.result_insts.push(pair);
            }
        }
    }

    fn record_vec_inst(&mut self, ty: &Type) {
        if let Type::Vec(inner) = ty {
            if !self.vec_insts.contains(ty) {
                self.vec_insts.push(ty.clone());
            }
            // `get`/`pop` возвращают `Option<T>` — тип нужен всегда.
            let opt = Type::Option(inner.clone());
            if !self.option_insts.contains(&opt) {
                self.option_insts.push(opt);
            }
        }
    }

    fn record_tuple_inst(&mut self, ty: &Type) {
        if let Type::Tuple(ts) = ty {
            if !self.tuple_insts.contains(ty) {
                self.tuple_insts.push(ty.clone());
            }
            for t in ts {
                self.record_tuple_inst(t);
            }
        }
    }

    fn record_slice_inst(&mut self, inner: &Type) {
        let ty = Type::Slice(Box::new(inner.clone()));
        if !self.slice_insts.contains(&ty) {
            self.slice_insts.push(ty);
        }
    }

    /// Рекурсивно записывает инстанциации `Option`/`Result`/`Vec` из типа.
    fn record_type_insts(&mut self, t: &Type) {
        match t {
            Type::Option(inner) => {
                self.record_type_insts(inner);
                if !self.option_insts.contains(t) {
                    self.option_insts.push(t.clone());
                }
            }
            Type::Result(ok, err) => {
                self.record_type_insts(ok);
                self.record_type_insts(err);
                let pair = ((**ok).clone(), (**err).clone());
                if !self.result_insts.contains(&pair) {
                    self.result_insts.push(pair);
                }
            }
            Type::Vec(inner) => {
                self.record_type_insts(inner);
                self.record_vec_inst(t);
            }
            Type::Ref(inner) => self.record_type_insts(inner),
            Type::Array(inner, _) => self.record_type_insts(inner),
            _ => {}
        }
    }

    /// Допустим ли тип как данные `Option`/`Result`.
    fn check_payload_type(&mut self, line: usize, t: &Type, wrapper: &str) {
        if !self.payload_type_ok(t) {
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "тип `{}` нельзя хранить в `{}`: допустимы int, bool, string, \
                     enum и Option/Result",
                    t.name(),
                    wrapper
                ),
            ));
        }
    }

    fn type_of_inner(&mut self, e: &Expr, ctx: &mut Ctx) -> Type {
        match &e.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::Str,
            ExprKind::Ident(name) if name == "None" => {
                self.diags.push(Diagnostic::new(
                    e.line,
                    "не удалось вывести тип `None`: укажите тип, \
                     например `let o Option<int> = None;`"
                        .to_string(),
                ));
                Type::Option(Box::new(Type::Int))
            }
            ExprKind::Ident(name) => {
                let Some(local) = ctx.scope.get(name).cloned() else {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("неизвестная переменная `{}`", name),
                    ));
                    return Type::Void;
                };
                if local.moved {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`{}` уже перемещён, использовать нельзя", name),
                    ));
                }
                // Использование ссылки: проверяем, что она ещё жива.
                if let Some(uid) = local.ref_uid {
                    let dead = self
                        .dead_ref(uid)
                        .map(|r| (r.created_line, r.killed.clone()));
                    if let Some((created_line, killed)) = dead {
                        let mut chain = String::new();
                        if created_line > 0 {
                            chain.push_str(&format!(", создана в строке {}", created_line));
                        }
                        if let Some((kline, reason)) = killed {
                            chain.push_str(&format!(
                                ", погашена в строке {}: {}",
                                kline, reason
                            ));
                        }
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "ссылка `{}` инвалидирована: объект `{}` был изменён или уничтожен{}",
                                name,
                                self.render(local.ref_target.as_ref().unwrap_or(&Path::root("?"))),
                                chain
                            ),
                        ));
                    }
                }
                local.ty
            }
            ExprKind::Variant { ty, name } => {
                let bad = self
                    .enums
                    .get(ty)
                    .filter(|e| !e.public && e.file != self.diags.file)
                    .map(|e| (e.line, e.file.clone()));
                if let Some((line, file)) = bad {
                    self.private(line, "enum", ty, &file);
                    return Type::Void;
                }
                match self.enums.get(ty) {
                Some(e) => match e.variants.iter().find(|v| v.name == *name) {
                    Some(v) if v.payload.is_empty() => Type::Named(ty.clone()),
                    Some(_) => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "вариант `{}::{}` несёт данные: нужен вызов `{}::{}(...)`",
                                ty, name, ty, name
                            ),
                        ));
                        Type::Named(ty.clone())
                    }
                    None => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("у enum `{}` нет варианта `{}`", ty, name),
                        ));
                        Type::Void
                    }
                },
                None => {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("неизвестный enum `{}`", ty),
                    ));
                    Type::Void
                }
                }
            }
            ExprKind::StructInit { name, fields } => {
                let bad = self
                    .structs
                    .get(name)
                    .filter(|s| !s.public && s.file != self.diags.file)
                    .map(|s| (s.line, s.file.clone()));
                if let Some((line, file)) = bad {
                    self.private(line, "структура", name, &file);
                    return Type::Void;
                }
                let Some(s) = self.structs.get(name).cloned() else {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("неизвестная структура `{}`", name),
                    ));
                    return Type::Void;
                };
                for f in &s.fields {
                    if !fields.iter().any(|(n, _)| n == &f.name) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("в конструкторе `{}` не задано поле `{}`", name, f.name),
                        ));
                    }
                }
                for (i, (n, _)) in fields.iter().enumerate() {
                    if fields[..i].iter().any(|(m, _)| m == n) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("поле `{}` задано дважды", n),
                        ));
                    }
                }
                for (n, val) in fields {
                    let Some(f) = s.fields.iter().find(|f| &f.name == n) else {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("у структуры `{}` нет поля `{}`", name, n),
                        ));
                        self.type_of(val, ctx);
                        continue;
                    };
                    let vt = self.type_of(val, ctx);
                    if !self.types_compatible(f.ty.clone(), &vt) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "поле `{}` ожидает `{}`, получено `{}`",
                                n,
                                f.ty.name(),
                                vt.name()
                            ),
                        ));
                    }
                }
                Type::Named(name.clone())
            },
            ExprKind::Field { obj, name } => {
                let base = self.type_of(obj, ctx);
                let base = base.deref().clone();
                let base_name = base.name();
                match base {
                    Type::Named(sn) => match self.structs.get(&sn) {
                        Some(s) => match s.fields.iter().find(|f| &f.name == name) {
                            Some(f) => f.ty.clone(),
                            None => {
                                self.diags.push(Diagnostic::new(
                                    e.line,
                                    format!("у структуры `{}` нет поля `{}`", sn, name),
                                ));
                                Type::Void
                            }
                        },
                        None => Type::Void,
                    },
                    Type::Tuple(ts) => match name.parse::<usize>() {
                        Ok(i) if i < ts.len() => ts[i].clone(),
                        _ => {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("у кортежа `{}` нет элемента `{}`", base_name, name),
                            ));
                            Type::Void
                        }
                    },
                    _ => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("`{}` — не структура, поле недоступно", base_name),
                        ));
                        Type::Void
                    }
                }
            }
            // `x.method(args)` — ищем метод по типу получателя и
            // проверяем как обычный вызов с `x` первым аргументом.
            ExprKind::MethodCall { obj, name, args } => {
                let obj_ty = self.type_of(obj, ctx).deref().clone();
                if let Type::Vec(elem) = &obj_ty {
                    return self.type_of_vec_method(e, obj, name, args, elem, ctx);
                }
                let Type::Named(tn) = obj_ty.clone() else {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("`{}` — не структура, метод недоступен", obj_ty.name()),
                    ));
                    return Type::Void;
                };
                let bad = self
                    .methods
                    .get(&(tn.clone(), name.clone()))
                    .filter(|fi| !fi.public && fi.file != self.diags.file)
                    .map(|fi| fi.file.clone());
                if let Some(file) = bad {
                    self.private(e.line, "метод", name, &file);
                    return Type::Void;
                }
                let Some(fi) = self.methods.get(&(tn.clone(), name.clone())).cloned() else {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!("у типа `{}` нет метода `{}`", tn, name),
                    ));
                    return Type::Void;
                };
                // Получатель передаётся первым аргументом. Если метод
                // ждёт ссылку, а получатель — значение, ссылка берётся
                // неявно: `p.sum()` читается как `Point::sum(&p)`.
                //
                // Фиктивный узел `Ref` здесь создавать нельзя: у парсера
                // и проверщика свои счётчики id, и новый id совпал бы с
                // id чужого выражения, испортив его тип. Вместо этого
                // факт неявной ссылки передаётся флагом.
                let wants_ref = fi
                    .params
                    .first()
                    .map(|p| matches!(p.kind, ParamKind::Ref | ParamKind::RefIn(_)))
                    .unwrap_or(false);
                let recv_is_ref = self
                    .expr_types
                    .get(&obj.id)
                    .map(|t| t.is_ref())
                    .unwrap_or(false);
                let auto_ref = wants_ref && !recv_is_ref;
                self.method_recv_ref.insert(e.id, auto_ref);
                let mut all = vec![(**obj).clone()];
                all.extend(args.iter().cloned());
                let recv_ty = self.expr_types.get(&obj.id).cloned();
                self.type_of_call_named_ext(name, &fi, &all, ctx, e.line, auto_ref, recv_ty)
            }
            ExprKind::Index { obj, idx } => {
                let base = self.type_of(obj, ctx);
                self.type_of(idx, ctx);
                let base = base.deref().clone();
                match base {
                    Type::Array(elem, _) => *elem,
                    Type::Vec(elem) => *elem,
                    Type::Slice(elem) => *elem,
                    _ => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("`{}` не является массивом", base.name()),
                        ));
                        Type::Void
                    }
                }
            }
            ExprKind::Array(items) => {
                let mut elem = Type::Void;
                for (i, it) in items.iter().enumerate() {
                    // Тип элемента — по первому элементу; остальные
                    // получают его как ожидаемый, чтобы `[Some(1), None]`
                    // вывел тип `None`.
                    let t = if i == 0 {
                        self.type_of(it, ctx)
                    } else {
                        self.type_of_expected(it, Some(&elem), ctx)
                    };
                    if i == 0 {
                        elem = t.clone();
                    } else if !self.types_compatible(elem.clone(), &t) {
                        self.diags.push(Diagnostic::new(
                            it.line,
                            format!(
                                "несовпадение типов элементов массива: ожидался `{}`, получен `{}`",
                                elem.name(),
                                t.name()
                            ),
                        ));
                    }
                    self.record_move(it, &t, ctx);
                }
                Type::Array(Box::new(elem), items.len())
            }
            ExprKind::Tuple(items) => {
                let mut ts = Vec::new();
                for it in items {
                    let t = self.type_of(it, ctx);
                    self.record_move(it, &t, ctx);
                    ts.push(t);
                }
                let ty = Type::Tuple(ts);
                self.record_tuple_inst(&ty);
                ty
            }
            ExprKind::Ref(inner) => {
                let t = self.type_of(inner, ctx);
                Type::Ref(Box::new(t))
            }
            ExprKind::Call { callee, args } => self.type_of_call(callee, args, ctx, e.line),
            ExprKind::Unary { op, e: inner } => {
                let t = self.type_of(inner, ctx);
                // Авторазыменование: `!r` где r: &bool — это `!(*r)`.
                let t = t.deref().clone();
                match op {
                    UnOp::Neg => {
                        if t != Type::Int {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("унарный минус требует int, получен `{}`", t.name()),
                            ));
                        }
                        Type::Int
                    }
                    UnOp::Not => {
                        if t != Type::Bool && t != Type::Void {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!("`!` требует bool, получен `{}`", t.name()),
                            ));
                        }
                        Type::Bool
                    }
                }
            }
            ExprKind::Binary { op, l, r } => {
                let lt = self.type_of(l, ctx);
                let rt = self.type_of(r, ctx);
                // Авторазыменование: сравнение ссылок сравнивает значения.
                let lt = lt.deref().clone();
                let rt = rt.deref().clone();
                match op {
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        // Enum с данными — структура в C; сравнивать её
                        // оператором нельзя. Разбирайте через `match`.
                        let payload_cmp = [&lt, &rt]
                            .into_iter()
                            .any(|t| self.is_payload_enum_type(t));
                        if payload_cmp {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                "сравнение enum с данными не поддерживается — \
                                 разберите значение через `match`"
                                    .to_string(),
                            ));
                        }
                        if [&lt, &rt].into_iter().any(|t| matches!(t, Type::Vec(_))) {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                "сравнение Vec не поддерживается — сравните длину и элементы"
                                    .to_string(),
                            ));
                        }
                        Type::Bool
                    }
                    BinOp::And | BinOp::Or => {
                        for (t, side) in [(&lt, "левый"), (&rt, "правый")] {
                            if *t != Type::Bool && *t != Type::Void {
                                self.diags.push(Diagnostic::new(
                                    e.line,
                                    format!(
                                        "`&&`/`||`: {} операнд должен быть bool, получен `{}`",
                                        side,
                                        t.name()
                                    ),
                                ));
                            }
                        }
                        Type::Bool
                    }
                    // Конкатенация строк: `+` для двух строк даёт строку.
                    BinOp::Add if lt == Type::Str && rt == Type::Str => Type::Str,
                    _ => {
                        if lt == Type::Float && rt == Type::Float {
                            return Type::Float;
                        }
                        if lt != Type::Int || rt != Type::Int {
                            self.diags.push(Diagnostic::new(
                                e.line,
                                format!(
                                    "арифметика требует int, получено `{}` и `{}`",
                                    lt.name(),
                                    rt.name()
                                ),
                            ));
                        }
                        Type::Int
                    }
                }
            }
            ExprKind::Closure { params, body } => {
                let mut param_tys = Vec::new();
                for p in params {
                    if p.ty == Type::Void {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("у параметра `{}` замыкания должен быть указан тип", p.name),
                        ));
                    }
                    param_tys.push(p.ty.clone());
                }
                let mut ctx = Ctx {
                    scope: HashMap::new(),
                    ret: Type::Void,
                    declared: Vec::new(),
                    loop_depth: 0,
                    loops: Vec::new(),
                    is_main: false,
                    diverged: false,
                };
                for p in params {
                    let uid = self.next_uid;
                    self.next_uid += 1;
                    self.names.insert(uid, p.name.clone());
                    ctx.scope.insert(
                        p.name.clone(),
                        Local {
                            uid,
                            ty: p.ty.clone(),
                            path: Path::root(&uid.to_string()),
                            ref_target: None,
                            ref_uid: None,
                            moved: false,
                            is_mut: false,
                            escapes: Vec::new(),
                        },
                    );
                }
                let ret = self.check_closure_body(body, &mut ctx);
                Type::Fn(param_tys, Box::new(ret))
            }
            ExprKind::If { cond, then_expr, else_expr } => {
                self.type_of(cond, ctx);
                // Ветки проверяем с одного состояния: move в then не
                // должен давать ложное «уже перемещён» в else.
                let base_scope = ctx.scope.clone();
                let base_refs = self.refs.clone();
                let tt = self.type_of(then_expr, ctx);
                let then_scope = ctx.scope.clone();
                let then_refs = self.refs.clone();
                ctx.scope = base_scope.clone();
                self.refs = base_refs.clone();
                let et = self.type_of(else_expr, ctx);
                let else_scope = ctx.scope.clone();
                let else_refs = self.refs.clone();
                let (scope, refs, _) = merge_if(
                    &base_scope,
                    &base_refs,
                    &then_scope,
                    &then_refs,
                    false,
                    &else_scope,
                    &else_refs,
                    false,
                );
                ctx.scope = scope;
                self.refs = refs;
                // Ветви должны давать одно значение: иначе результат
                // `if` зависит от того, какая ветвь выполнилась, и тип
                // выражения становится неопределённым.
                if !self.types_compatible(tt.clone(), &et) {
                    self.diags.push(Diagnostic::new(
                        e.line,
                        format!(
                            "несовпадение типов в ветвях `if`: `{}` и `{}`",
                            tt.name(),
                            et.name()
                        ),
                    ));
                }
                // Ветвь забирает значение — значит, перемещение. Ветви
                // обе: выполнится только одна, но после `if` обе
                // переменные считаются перемещёнными (как в Rust).
                if !tt.is_ref() {
                    self.record_move(then_expr, &tt, ctx);
                }
                if !et.is_ref() {
                    self.record_move(else_expr, &et, ctx);
                }
                tt
            }
        }
    }

    fn type_of_call(&mut self, callee: &Expr, args: &[Expr], ctx: &mut Ctx, line: usize) -> Type {
        let ExprKind::Ident(name) = &callee.kind else {
            self.diags.push(Diagnostic::new(line, "вызов: поддерживается только имя функции".into()));
            return Type::Void;
        };

        // Вызов замыкания: `f(args)`, где `f` — локальная типа `fn(...)`.
        if let Some(local) = ctx.scope.get(name) {
            if let Type::Fn(param_tys, ret) = &local.ty {
                self.expr_types.insert(callee.id, local.ty.clone());
                let param_tys = param_tys.clone();
                let ret = (**ret).clone();
                if args.len() != param_tys.len() {
                    self.diags.push(Diagnostic::new(
                        line,
                        format!(
                            "замыкание `{}` принимает {} аргументов, передано {}",
                            name,
                            param_tys.len(),
                            args.len()
                        ),
                    ));
                }
                for (a, pt) in args.iter().zip(&param_tys) {
                    let at = self.type_of(a, ctx);
                    if !self.types_compatible(pt.clone(), &at) {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!(
                                "аргумент замыкания: ожидался `{}`, получен `{}`",
                                pt.name(),
                                at.name()
                            ),
                        ));
                    }
                }
                return ret;
            }
        }

        // `Vec::new()` без ожидаемого типа: элемент не выводится.
        if name == "Vec::new" {
            self.diags.push(Diagnostic::new(
                line,
                "не удалось вывести тип `Vec::new`: укажите тип, \
                 например `let v Vec<int> = Vec::new();`"
                    .to_string(),
            ));
            let ty = Type::Vec(Box::new(Type::Int));
            self.record_vec_inst(&ty);
            return ty;
        }

        // Встроенные конструкторы Option/Result. Здесь — без ожидаемого
        // типа: `Some` выводит T из аргумента, `Ok`/`Err` без контекста
        // не могут вывести второй параметр и сообщают об этом.
        if name == "Some" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`Some` принимает один аргумент, получено {}", args.len()),
                ));
            }
            let at = match args.first() {
                Some(a) => self.type_of(a, ctx),
                None => Type::Int,
            };
            self.check_payload_type(line, &at, "Option");
            let ty = Type::Option(Box::new(at));
            self.record_option_inst(&ty);
            return ty;
        }
        if name == "Ok" || name == "Err" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`{}` принимает один аргумент, получено {}", name, args.len()),
                ));
            }
            let at = match args.first() {
                Some(a) => self.type_of(a, ctx),
                None => Type::Int,
            };
            self.check_payload_type(line, &at, "Result");
            let ty = if name == "Ok" {
                self.diags.push(Diagnostic::new(
                    line,
                    "не удалось вывести тип ошибки `Ok`: укажите тип, \
                     например `let r Result<int, string> = Ok(5);`"
                        .to_string(),
                ));
                Type::Result(Box::new(at), Box::new(Type::Str))
            } else {
                self.diags.push(Diagnostic::new(
                    line,
                    "не удалось вывести тип значения `Err`: укажите тип, \
                     например `let r Result<int, string> = Err(\"bad\");`"
                        .to_string(),
                ));
                Type::Result(Box::new(Type::Int), Box::new(at))
            };
            self.record_result_inst(&ty);
            return ty;
        }

        // Конструктор варианта enum: `Option::Some(5)`.
        if let Some((ty, variant)) = name.split_once("::") {
            let bad = self
                .enums
                .get(ty)
                .filter(|e| !e.public && e.file != self.diags.file)
                .map(|e| (e.line, e.file.clone()));
            if let Some((line, file)) = bad {
                self.private(line, "enum", ty, &file);
                return Type::Void;
            }
            if let Some(e) = self.enums.get(ty).cloned() {
                match e.variants.iter().find(|v| v.name == variant).cloned() {
                    Some(v) => {
                        if args.len() != v.payload.len() {
                            self.diags.push(Diagnostic::new(
                                line,
                                format!(
                                    "вариант `{}::{}` ждёт {}, передан {}",
                                    ty,
                                    variant,
                                    аргументов(v.payload.len()),
                                    args.len()
                                ),
                            ));
                        }
                        for (a, pt) in args.iter().zip(v.payload.iter()) {
                            let at = self.type_of_expected(a, Some(pt), ctx);
                            if !self.types_compatible(pt.clone(), &at) {
                                self.diags.push(Diagnostic::new(
                                    a.line,
                                    format!(
                                        "аргумент варианта `{}::{}`: ожидался `{}`, получен `{}`",
                                        ty,
                                        variant,
                                        pt.name(),
                                        at.name()
                                    ),
                                ));
                            }
                        }
                        return Type::Named(ty.to_string());
                    }
                    None => {
                        self.diags.push(Diagnostic::new(
                            line,
                            format!("у enum `{}` нет варианта `{}`", ty, variant),
                        ));
                        return Type::Named(ty.to_string());
                    }
                }
            }
        }

        // Конструктор структуры?
        let bad = self
            .structs
            .get(name)
            .filter(|s| !s.public && s.file != self.diags.file)
            .map(|s| (s.line, s.file.clone()));
        if let Some((line, file)) = bad {
            self.private(line, "структура", name, &file);
            return Type::Void;
        }
        if let Some(s) = self.structs.get(name).cloned() {
            if args.len() != s.fields.len() {
                self.diags.push(Diagnostic::new(
                    line,
                    format!(
                        "конструктор `{}` ждёт {}, передан {}",
                        name,
                        аргументов(s.fields.len()),
                        args.len()
                    ),
                ));
            }
            for (a, f) in args.iter().zip(s.fields.iter()) {
                let at = self.type_of_expected(a, Some(&f.ty), ctx);
                if !self.types_compatible(f.ty.clone(), &at) {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!(
                            "аргумент `{}`: ожидался `{}`, получен `{}`",
                            f.name,
                            f.ty.name(),
                            at.name()
                        ),
                    ));
                }
                self.record_move(a, &at, ctx);
            }
            return Type::Named(name.clone());
        }

        // Встроенная функция?
        // `print` печатает без перевода строки, `println` — с переводом.
        if name == "print" || name == "println" {
            for a in args {
                let at = self.type_of(a, ctx);
                let base = at.deref().clone();
                let is_payload = self.is_payload_enum_type(&base);
                if is_payload {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        "печать enum с данными не поддерживается — \
                         разберите значение через `match`"
                            .to_string(),
                    ));
                }
                if matches!(base, Type::Vec(_)) {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        "печать Vec не поддерживается — обойдите элементы через `len` и `v[i]`"
                            .to_string(),
                    ));
                }
                self.record_move(a, &at, ctx);
            }
            return Type::Void;
        }

        // `len` — длина строки или массива. Для массива значение известно
        // на этапе компиляции (размер — часть типа), для строки считается
        // в рантайме через `strlen`.
        if name == "len" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`len` принимает один аргумент, получено {}", args.len()),
                ));
            }
            if let Some(a) = args.first() {
                let t = self.type_of(a, ctx);
                let inner = match &t {
                    Type::Ref(inner) => inner.as_ref(),
                    other => other,
                };
                if !matches!(inner, Type::Str | Type::Array(..) | Type::Vec(_) | Type::Slice(_)) {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!(
                            "`len` работает со строкой, массивом или Vec, получен `{}`",
                            t.name()
                        ),
                    ));
                }
            }
            return Type::Int;
        }

        // `read_line` — строка из stdin. Память под неё берётся из кучи
        // и не освобождается (см. волну 8).
        if name == "read_line" {
            if !args.is_empty() {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`read_line` не принимает аргументов, получено {}", args.len()),
                ));
            }
            return Type::Str;
        }

        // `read_int` — целое из stdin. Читает строку и разбирает её,
        // поэтому не оставляет мусора в буфере ввода (в отличие от
        // `scanf`), и `read_line` после него работает ожидаемо.
        if name == "read_int" {
            if !args.is_empty() {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`read_int` не принимает аргументов, получено {}", args.len()),
                ));
            }
            return Type::Int;
        }

        // `argc` — сколько аргументов получила программа, включая
        // имя самой программы (как в C).
        if name == "argc" {
            if !args.is_empty() {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`argc` не принимает аргументов, получено {}", args.len()),
                ));
            }
            return Type::Int;
        }

        // `argv(i)` — i-й аргумент командной строки. `argv(0)` — имя
        // программы. Выход за границы не проверяется: размер массива
        // известен только в рантайме (в Rust это `args()` с `Option`).
        if name == "argv" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`argv` принимает один аргумент, получено {}", args.len()),
                ));
            }
            if let Some(a) = args.first() {
                let t = self.type_of(a, ctx);
                if t != Type::Int {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`argv` ждёт номер аргумента типа `int`, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            return Type::Str;
        }

        // `open(path, mode)` — открывает файл. Возвращает fd или -1.
        // Режим — строка как в C: "r", "w", "a".
        if name == "open" {
            if args.len() != 2 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!(
                        "`open` принимает два аргумента (путь и режим), получено {}",
                        args.len()
                    ),
                ));
            }
            for a in args {
                let t = self.type_of(a, ctx);
                if t != Type::Str {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`open` ждёт строку, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            return Type::Int;
        }

        // `read(fd)` — читает файл целиком в строку.
        if name == "read" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`read` принимает один аргумент (fd), получено {}", args.len()),
                ));
            }
            if let Some(a) = args.first() {
                let t = self.type_of(a, ctx);
                if t != Type::Int {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`read` ждёт fd типа `int`, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            return Type::Str;
        }

        // `write(fd, s)` — пишет строку в файл.
        if name == "write" {
            if args.len() != 2 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!(
                        "`write` принимает два аргумента (fd и строку), получено {}",
                        args.len()
                    ),
                ));
            }
            if let Some(a) = args.first() {
                let t = self.type_of(a, ctx);
                if t != Type::Int {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`write` ждёт fd типа `int`, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            if let Some(a) = args.get(1) {
                let t = self.type_of(a, ctx);
                if t != Type::Str {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`write` ждёт строку, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            return Type::Void;
        }

        // `close(fd)` — закрывает файл.
        if name == "close" {
            if args.len() != 1 {
                self.diags.push(Diagnostic::new(
                    line,
                    format!("`close` принимает один аргумент (fd), получено {}", args.len()),
                ));
            }
            if let Some(a) = args.first() {
                let t = self.type_of(a, ctx);
                if t != Type::Int {
                    self.diags.push(Diagnostic::new(
                        a.line,
                        format!("`close` ждёт fd типа `int`, получен `{}`", t.name()),
                    ));
                }
                self.record_move(a, &t, ctx);
            }
            return Type::Void;
        }

        let bad = self
            .funcs
            .get(name)
            .filter(|fi| !fi.public && fi.file != self.diags.file)
            .map(|fi| fi.file.clone());
        if let Some(file) = bad {
            self.private(line, "функция", name, &file);
            return Type::Void;
        }
        let Some(fi) = self.funcs.get(name).cloned() else {
            self.diags.push(Diagnostic::new(line, format!("неизвестная функция `{}`", name)));
            return Type::Void;
        };

        self.type_of_call_named(name, &fi, args, ctx, line)
    }

    /// Общая часть проверки вызова: арность, типы аргументов, перенос
    /// mut-контракта на пути вызывающего.
    fn type_of_call_named(
        &mut self,
        name: &str,
        fi: &FuncInfo,
        args: &[Expr],
        ctx: &mut Ctx,
        line: usize,
    ) -> Type {
        self.type_of_call_named_ext(name, fi, args, ctx, line, false, None)
    }

    /// То же, но с флагом `auto_ref`: первый аргумент — получатель
    /// метода, которому ссылка берётся автоматически, поэтому
    /// требовать `&` от вызывающего не нужно.
    fn type_of_call_named_ext(
        &mut self,
        name: &str,
        fi: &FuncInfo,
        args: &[Expr],
        ctx: &mut Ctx,
        line: usize,
        auto_ref: bool,
        recv_ty: Option<Type>,
    ) -> Type {
        if args.len() != fi.params.len() {
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "`{}` ждёт {}, передан {}",
                    name,
                    аргументов(fi.params.len()),
                    args.len()
                ),
            ));
        }

        // Для каждого аргумента вычисляем его путь — он нужен, чтобы
        // перенести mut-контракт вызываемой функции на вызывающего.
        let mut arg_paths: Vec<Option<Path>> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let param = fi.params.get(i);
            // Получатель метода уже проверен в `MethodCall`: повторный
            // `type_of` увидел бы его перемещённым и дал ложную ошибку.
            let t = if i == 0 && recv_ty.is_some() {
                recv_ty.clone().unwrap()
            } else {
                match param {
                    Some(p) if matches!(p.kind, ParamKind::Owned) => {
                        self.type_of_expected(a, Some(&p.ty), ctx)
                    }
                    _ => self.type_of(a, ctx),
                }
            };
            // Путь есть только у «мест» (переменных и их полей).
            let p = self.place_path_of(a, ctx);
            arg_paths.push(p);

            let Some(param) = param else { continue };
            match param.kind {
                ParamKind::Owned => {
                    // Тип проверяется и здесь: `take(&u)` там, где ждут
                    // значение, — ошибка. Без этой проверки она всплывала
                    // только в C-компиляторе, где причина не видна.
                    if !self.types_compatible(param.ty.clone(), &t) {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!(
                                "параметр `{}` ожидает `{}`, получен `{}`",
                                param.name,
                                param.ty.name(),
                                t.name()
                            ),
                        ));
                    }
                    // Передача владения — перемещение.
                    if !self.is_copy(&t) {
                        self.record_move(a, &t, ctx);
                    }
                }
                ParamKind::Ref | ParamKind::RefIn(_) => {
                    // Получатель метода: ссылка берётся автоматически.
                    if auto_ref && i == 0 {
                        continue;
                    }
                    if !t.is_ref() {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!(
                                "параметр `{}` ожидает ссылку, получен `{}`",
                                param.name,
                                t.name()
                            ),
                        ));
                    }
                }
            }
        }

        // Применяем mut-контракт вызываемой функции.
        // Пути контракта заданы в пространстве имён параметров вызываемой
        // функции; переносим их в пространство вызывающего через аргументы.
        for mp in &fi.mut_paths {
            let Some(idx) = fi.params.iter().position(|p| p.name == mp.param) else {
                continue;
            };
            let Some(Some(base)) = arg_paths.get(idx).cloned() else {
                continue;
            };
            let effective = Path { root: base.root.clone(), segs: mp.segs.clone() };
            // Параметр по значению перемещён в функцию — его изменение
            // вызывающему не видно, поэтому `mut` у владельца не нужен.
            if !matches!(fi.params[idx].kind, ParamKind::Owned) {
                self.check_owner_is_mut(&effective, line, ctx);
            }
            self.modified.push((effective.clone(), false));
            self.invalidate_inside(&effective, line, "вызов функции с `mut(...)`");
        }
        // Параметры, помеченные `mut`, тоже объявляют мутацию.
        for (i, p) in fi.params.iter().enumerate() {
            if !p.mutable {
                continue;
            }
            let Some(Some(base)) = arg_paths.get(i).cloned() else {
                continue;
            };
            let effective = match &p.kind {
                ParamKind::RefIn(_) => {
                    // Для параметра-внутри действует фактический путь
                    // переданного элемента: мутация затрагивает его поля,
                    // но не сам элемент.
                    base.clone()
                }
                _ => Path { root: base.root.clone(), segs: Vec::new() },
            };
            // Своя копия параметра по значению: вызывающий не при чём.
            if !matches!(p.kind, ParamKind::Owned) {
                self.check_owner_is_mut(&effective, line, ctx);
            }
            self.modified.push((effective.clone(), false));
            self.invalidate_inside(&effective, line, "вызов функции с `mut`-параметром");
        }

        // Если вызов мутирует параметр и получает ссылку на локальную,
        // эта ссылка может осесть в параметре и пережить кадр:
        // `store(h, &i)` кладёт `&i` в `h.r`. Мутируют и `mut(...)`,
        // и параметры с пометкой `mut`.
        let mut mutated: Vec<(usize, String, Vec<Seg>)> = Vec::new();
        for mp in &fi.mut_paths {
            if let Some(idx) = fi.params.iter().position(|p| p.name == mp.param) {
                mutated.push((idx, mp.param.clone(), mp.segs.clone()));
            }
        }
        for (i, p) in fi.params.iter().enumerate() {
            if p.mutable {
                mutated.push((i, p.name.clone(), Vec::new()));
            }
        }
        for (idx, pname, msegs) in mutated {
            let Some(Some(base)) = arg_paths.get(idx).cloned() else {
                continue;
            };
            for (j, a) in args.iter().enumerate() {
                if j == idx {
                    continue;
                }
                for (_, target) in self.escapes_in(a, ctx) {
                    if !self.path_is_local(&target, ctx) {
                        continue;
                    }
                    if let Ok(uid) = base.root.parse::<u32>() {
                        // Локальный контейнер: ссылка оседает в его
                        // escapes, и `return h` её увидит.
                        let mut segs = base.segs.clone();
                        segs.extend(msegs.clone());
                        if let Some(l) = ctx.scope.values_mut().find(|l| l.uid == uid) {
                            if !l.escapes.iter().any(|(s, t)| s == &segs && t == &target) {
                                l.escapes.push((segs, target.clone()));
                            }
                        }
                    } else {
                        self.diags.push(Diagnostic::new(
                            a.line,
                            format!(
                                "вызов `{}` сохраняет ссылку на локальную переменную `{}` в параметре `{}` — она перестанет существовать после возврата",
                                name,
                                self.render(&target),
                                pname
                            ),
                        ));
                    }
                }
            }
        }

        fi.ret.clone()
    }

    /// Владелец пути, который собирается изменить вызываемая функция,
    /// должен быть объявлен `mut`.
    ///
    /// Прямая запись (`p.x = 1`) проверяется в `check_assign`. Но то же
    /// изменение можно выполнить через вызов — `p.shift()` или
    /// `shift(&p)`, — и тогда проверка обязана стоять здесь, иначе
    /// неизменяемая переменная меняется в обход правил. Ровно это Rust
    /// запрещает как E0596 («cannot borrow as mutable, as it is not
    /// declared as mutable»).
    ///
    /// Проверяются только пути, чей корень — локальная переменная.
    /// Пути, начинающиеся с имени параметра, — забота контракта самого
    /// параметра, а не вызывающего.
    fn check_owner_is_mut(&mut self, path: &Path, line: usize, ctx: &Ctx) {
        let Ok(uid) = path.root.parse::<u32>() else {
            return;
        };
        // Владелец — переменная, чей путь является префиксом пути записи.
        // Для ссылки на элемент это контейнер, а не сама ссылка.
        let Some(owner) = ctx
            .scope
            .values()
            .filter(|l| l.ref_target.is_none())
            .find(|l| l.path.is_prefix_of(path))
            .cloned()
        else {
            return;
        };
        let _ = uid;
        if !owner.is_mut {
            let name = self.names.get(&owner.uid).cloned().unwrap_or_default();
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "`{}` не объявлен как `mut` — вызов изменяет его, \
                     а изменять неизменяемую переменную нельзя",
                    name
                ),
            ));
        }
    }

    /// Путь значения выражения, если это «место» (переменная, поле, элемент).
    fn place_path_of(&mut self, e: &Expr, ctx: &mut Ctx) -> Option<Path> {
        match &e.kind {
            ExprKind::Ident(name) => {
                let local = ctx.scope.get(name).cloned()?;
                match local.ref_target {
                    Some(t) => Some(t),
                    None => Some(local.path),
                }
            }
            ExprKind::Field { obj, name } => {
                let p = self.place_path_of(obj, ctx)?;
                Some(p.child(Seg::Field(name.clone())))
            }
            ExprKind::Index { obj, idx } => {
                let p = self.place_path_of(obj, ctx)?;
                match &idx.kind {
                    ExprKind::Int(n) => Some(p.child(Seg::Idx(*n))),
                    _ => Some(p.child(Seg::Elems)),
                }
            }
            ExprKind::Ref(inner) => self.place_path_of(inner, ctx),
            ExprKind::Array(_) => None,
            _ => None,
        }
    }

    /// Все цели ссылки-выражения. У локальной ссылки их может быть
    /// несколько (`let r = pick(&x, &y)`), и цепочка `let q = id(r)`
    /// должна унаследовать их все, а не только первую.
    fn ref_targets_of(&mut self, e: &Expr, ctx: &mut Ctx) -> Vec<Path> {
        if let ExprKind::Ident(name) = &e.kind {
            if let Some(l) = ctx.scope.get(name) {
                if let Some(uid) = l.ref_uid {
                    if let Some(r) = self.refs.iter().find(|r| r.uid == uid) {
                        return r.targets.clone();
                    }
                }
            }
        }
        self.place_path_of(e, ctx).into_iter().collect()
    }

    /// Цели ссылки-результата вызова: пути ref-аргументов. Без
    /// lifetime-аннотаций точнее сказать нельзя.
    fn call_ref_target(&mut self, e: &Expr, ctx: &mut Ctx) -> Option<Vec<Path>> {
        let mut out = Vec::new();
        match &e.kind {
            ExprKind::Call { callee, args } => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return None;
                };
                let Some(fi) = self.funcs.get(name).cloned() else {
                    return None;
                };
                for (i, a) in args.iter().enumerate() {
                    let is_ref = fi
                        .params
                        .get(i)
                        .map(|p| matches!(p.kind, ParamKind::Ref | ParamKind::RefIn(_)))
                        .unwrap_or(false);
                    if is_ref {
                        out.extend(self.ref_targets_of(a, ctx));
                    }
                }
            }
            ExprKind::MethodCall { obj, name, args } => {
                let tn = self
                    .expr_types
                    .get(&obj.id)
                    .map(|t| t.deref().name())
                    .unwrap_or_default();
                let Some(fi) = self.methods.get(&(tn, name.clone())).cloned() else {
                    return None;
                };
                if fi
                    .params
                    .first()
                    .map(|p| matches!(p.kind, ParamKind::Ref | ParamKind::RefIn(_)))
                    .unwrap_or(false)
                {
                    out.extend(self.ref_targets_of(obj, ctx));
                }
                for (i, a) in args.iter().enumerate() {
                    let is_ref = fi
                        .params
                        .get(i + 1)
                        .map(|p| matches!(p.kind, ParamKind::Ref | ParamKind::RefIn(_)))
                        .unwrap_or(false);
                    if is_ref {
                        out.extend(self.ref_targets_of(a, ctx));
                    }
                }
            }
            _ => return None,
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Цели ссылки из `if`: пути обеих ветвей.
    fn if_ref_target(&mut self, e: &Expr, ctx: &mut Ctx) -> Option<Vec<Path>> {
        let ExprKind::If { then_expr, else_expr, .. } = &e.kind else {
            return None;
        };
        let mut out = Vec::new();
        out.extend(self.ref_targets_of(then_expr, ctx));
        out.extend(self.ref_targets_of(else_expr, ctx));
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    // --------------------------------------------------------
    // Анализ выхода (escape analysis)
    // --------------------------------------------------------
    //
    // Ссылка не должна пережить кадр, в котором создана. Проверяем
    // это на границе функции: значение, содержащее ссылку на локальную
    // переменную, не может быть возвращено наружу.
    //
    // Почему это нужно. Путь ссылки привязан к локальной переменной,
    // а переменная умирает вместе с кадром. Ссылка, уехавшая в
    // структуре, переживает кадр — и чтение поля после возврата даёт
    // мусор (use-after-free). Компилятор C такое не ловит: он видит
    // обычный указатель.

    /// Ссылки на локальные переменные, спрятанные внутри значения.
    ///
    /// Возвращает список пар «путь внутри значения → цель ссылки».
    /// Для `H(&i)` это `[([Field("r")], i)]`: в поле `r` лежит ссылка
    /// на локальную `i`.
    ///
    /// Ссылка на ПАРАМЕТР сюда не попадает: параметр живёт в кадре
    /// вызывающего, а не в нашем, поэтому возвращать его наружу
    /// безопасно.
    fn escapes_in(&mut self, e: &Expr, ctx: &mut Ctx) -> Vec<Escape> {
        // Значение, чей тип не содержит ссылок, ничего утащить не может.
        // Без этого `return r.x` (r: &S, x: int) давал ложную ошибку:
        // утечка `r` → `s` добавляла сегмент `x`, хотя `x` — копия int.
        if let Some(t) = self.expr_types.get(&e.id) {
            if !self.type_contains_ref(t) {
                return Vec::new();
            }
        }
        match &e.kind {
            // Ссылка на локальную переменную — утечка.
            ExprKind::Ref(inner) => {
                let Some(path) = self.place_path_of(inner, ctx) else {
                    return Vec::new();
                };
                if self.path_is_local(&path, ctx) {
                    vec![(Vec::new(), path)]
                } else {
                    Vec::new()
                }
            }
            // Переменная: её собственные утечки. Если это ссылка,
            // `escapes` уже содержит её саму.
            ExprKind::Ident(name) => {
                let Some(local) = ctx.scope.get(name).cloned() else {
                    return Vec::new();
                };
                // Ссылка-переменная: цель лежит прямо в ней.
                if let Some(t) = local.ref_target {
                    if self.path_is_local(&t, ctx) {
                        return vec![(Vec::new(), t)];
                    }
                    return Vec::new();
                }
                local.escapes
            }
            // Поле: утечки объекта, у которых этот сегмент — первый.
            //
            // Если первым сегментом утечки стоит именно это поле, значит
            // мы достали саму ссылку — сегмент «схлопывается». Иначе
            // в этом поле ссылки нет, и утечка не проходит.
            ExprKind::Field { obj, name } => {
                let seg = Seg::Field(name.clone());
                self.escapes_in(obj, ctx)
                    .into_iter()
                    .filter_map(|(mut segs, t)| {
                        if segs.is_empty() {
                            // Само значение — ссылка; поле читается
                            // внутри неё.
                            segs.insert(0, seg.clone());
                            Some((segs, t))
                        } else if segs[0] == seg {
                            segs.remove(0);
                            Some((segs, t))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            // Индекс: то же, с сегментом индекса.
            ExprKind::Index { obj, idx } => {
                let seg = match &idx.kind {
                    ExprKind::Int(n) => Seg::Idx(*n),
                    _ => Seg::Elems,
                };
                self.escapes_in(obj, ctx)
                    .into_iter()
                    .filter_map(|(mut segs, t)| {
                        if segs.is_empty() {
                            segs.insert(0, seg.clone());
                            Some((segs, t))
                        } else if segs[0] == seg {
                            segs.remove(0);
                            Some((segs, t))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            // Конструктор структуры: утечки каждого аргумента
            // с именем соответствующего поля.
            ExprKind::Call { callee, args } => {
                let ExprKind::Ident(cname) = &callee.kind else {
                    return Vec::new();
                };
                if let Some(s) = self.structs.get(cname).cloned() {
                    let mut out = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        let Some(field) = s.fields.get(i) else { break };
                        for (mut segs, t) in self.escapes_in(a, ctx) {
                            segs.insert(0, Seg::Field(field.name.clone()));
                            out.push((segs, t));
                        }
                    }
                    return out;
                }
                // Обычный вызов: без lifetime-аннотаций результат может
                // указывать в любой ref-аргумент. Консервативно объединяем
                // утечки всех аргументов — иначе `return id(&i)` уносит
                // висячий указатель.
                let mut out = Vec::new();
                for a in args {
                    out.extend(self.escapes_in(a, ctx));
                }
                out
            }
            // Именованный конструктор: утечки каждого поля.
            ExprKind::StructInit { fields, .. } => {
                let mut out = Vec::new();
                for (fname, a) in fields {
                    for (mut segs, t) in self.escapes_in(a, ctx) {
                        segs.insert(0, Seg::Field(fname.clone()));
                        out.push((segs, t));
                    }
                }
                out
            }
            // Литерал массива: утечки каждого элемента.
            ExprKind::Array(items) => {
                let mut out = Vec::new();
                for (i, a) in items.iter().enumerate() {
                    for (mut segs, t) in self.escapes_in(a, ctx) {
                        segs.insert(0, Seg::Idx(i as i64));
                        out.push((segs, t));
                    }
                }
                out
            }
            // Кортеж: утечки каждого элемента.
            ExprKind::Tuple(items) => {
                let mut out = Vec::new();
                for (i, a) in items.iter().enumerate() {
                    for (mut segs, t) in self.escapes_in(a, ctx) {
                        segs.insert(0, Seg::Field(i.to_string()));
                        out.push((segs, t));
                    }
                }
                out
            }
            // Ветвление: объединяем утечки обеих ветвей.
            ExprKind::If { then_expr, else_expr, .. } => {
                let mut out = self.escapes_in(then_expr, ctx);
                out.extend(self.escapes_in(else_expr, ctx));
                out
            }
            // Вызов метода: результат может указывать в получателя
            // или в любой ref-аргумент.
            ExprKind::MethodCall { obj, args, .. } => {
                let mut out = self.escapes_in(obj, ctx);
                for a in args {
                    out.extend(self.escapes_in(a, ctx));
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// Содержит ли тип ссылку — сам или внутри (поле, элемент, вариант).
    fn type_contains_ref(&self, t: &Type) -> bool {
        self.type_contains_ref_depth(t, 0)
    }

    fn type_contains_ref_depth(&self, t: &Type, depth: usize) -> bool {
        // Рекурсивная структура (`struct A { a A }`) не должна
        // переполнять стек компилятора.
        if depth > 32 {
            return false;
        }
        match t {
            Type::Ref(_) | Type::Slice(_) => true,
            Type::Array(inner, _) | Type::Vec(inner) | Type::Option(inner) => {
                self.type_contains_ref_depth(inner, depth + 1)
            }
            Type::Result(ok, err) => {
                self.type_contains_ref_depth(ok, depth + 1)
                    || self.type_contains_ref_depth(err, depth + 1)
            }
            Type::Tuple(ts) => ts.iter().any(|t| self.type_contains_ref_depth(t, depth + 1)),
            Type::Named(n) => {
                if let Some(s) = self.structs.get(n) {
                    return s
                        .fields
                        .iter()
                        .any(|f| self.type_contains_ref_depth(&f.ty, depth + 1));
                }
                if let Some(e) = self.enums.get(n) {
                    return e.variants.iter().any(|v| {
                        v.payload
                            .iter()
                            .any(|t| self.type_contains_ref_depth(t, depth + 1))
                    });
                }
                false
            }
            _ => false,
        }
    }

    /// Указывает ли путь на локальную переменную (а не на параметр).
    ///
    /// Корень пути локальной переменной — её числовой uid; корень
    /// параметра — его имя. Это и различает случаи: вернуть ссылку
    /// на параметр можно, на локальную — нет.
    fn path_is_local(&self, p: &Path, _ctx: &Ctx) -> bool {
        p.root.parse::<u32>().is_ok()
    }

    /// Проверяет, что возвращаемое значение не тащит наружу ссылку
    /// на локальную переменную.
    fn check_return_escapes(&mut self, e: &Expr, ctx: &mut Ctx) {
        let escapes = self.escapes_in(e, ctx);
        for (segs, target) in escapes {
            let where_ = if segs.is_empty() {
                "возвращаемое значение".to_string()
            } else {
                format!("поле `{}` возвращаемого значения", self.render_segs(&segs))
            };
            self.diags.push(Diagnostic::new(
                e.line,
                format!(
                    "{} содержит ссылку на локальную переменную `{}` — \
                     она перестанет существовать после возврата; \
                     вернуть можно только ссылку на параметр",
                    where_,
                    self.render(&target)
                ),
            ));
        }
    }

    /// Путь внутри значения в читаемом виде: `r`, `items[0].r`.
    fn render_segs(&self, segs: &[Seg]) -> String {
        let mut s = String::new();
        for seg in segs {
            match seg {
                Seg::Field(f) => {
                    if !s.is_empty() {
                        s.push('.');
                    }
                    s.push_str(f);
                }
                Seg::Idx(i) => s.push_str(&format!("[{}]", i)),
                Seg::Elems => s.push_str("[]"),
            }
        }
        s
    }

    /// Помечает источник перемещённым, если он не Copy.
    ///
    /// Перемещать можно только из локальной переменной — целого
    /// владельца. Из поля структуры или элемента коллекции перемещать
    /// нельзя: в контейнере осталась бы дыра — место, за которое уже
    /// никто не отвечает. Rust запрещает то же самое («cannot move out
    /// of index»).
    ///
    /// Копируемые типы (int, bool, string, &T) проблемы не имеют:
    /// чтение `a[0].name` копирует значение, а не забирает его.
    fn record_move(&mut self, e: &Expr, ty: &Type, ctx: &mut Ctx) {
        if self.is_copy(ty) {
            return;
        }
        match &e.kind {
            ExprKind::Ident(name) => {
                let path = ctx.scope.get(name).map(|l| l.path.clone());
                if let Some(l) = ctx.scope.get_mut(name) {
                    l.moved = true;
                }
                // Перемещение уничтожает значение: ссылки на него гаснут.
                if let Some(path) = path {
                    self.invalidate_replace(&path, e.line, "перемещение");
                }
            }
            ExprKind::Field { .. } | ExprKind::Index { .. } => {
                let place = self
                    .place_path_of(e, ctx)
                    .map(|p| self.render(&p))
                    .unwrap_or_default();
                let where_ = if matches!(&e.kind, ExprKind::Field { .. }) {
                    "в структуре"
                } else {
                    "в коллекции"
                };
                self.diags.push(Diagnostic::new(
                    e.line,
                    format!(
                        "перемещение из `{}` запрещено: {} осталась бы дыра — значение, \
                         которого больше нет. Возьмите ссылку `&{}`",
                        place, where_, place
                    ),
                ));
            }
            ExprKind::StructInit { fields, .. } => {
                let vals: Vec<(Expr, Type)> = fields
                    .iter()
                    .map(|(_, v)| (v.clone(), self.type_of(v, ctx)))
                    .collect();
                for (v, vt) in vals {
                    self.record_move(&v, &vt, ctx);
                }
            }
            ExprKind::Tuple(items) => {
                let vals: Vec<(Expr, Type)> = items
                    .iter()
                    .map(|v| (v.clone(), self.type_of(v, ctx)))
                    .collect();
                for (v, vt) in vals {
                    self.record_move(&v, &vt, ctx);
                }
            }
            _ => {}
        }
    }

    /// Допустим ли тип в данных варианта enum.
    ///
    /// Пока только копируемые типы: int, bool, string и другой enum.
    /// Структуры и массивы отложены — они не Copy, и деструктуризация
    /// в `match` означала бы перемещение из enum.
    fn payload_type_ok(&self, t: &Type) -> bool {
        match t {
            Type::Int | Type::Bool | Type::Str => true,
            Type::Named(n) => self.enums.contains_key(n),
            Type::Option(inner) => self.payload_type_ok(inner),
            Type::Result(ok, err) => self.payload_type_ok(ok) && self.payload_type_ok(err),
            _ => false,
        }
    }

    /// Есть ли цикл в данных enum'ов: `enum E { A(E) }`.
    fn enum_has_cycle(&self, name: &str, visiting: &mut Vec<String>) -> bool {
        if visiting.iter().any(|v| v == name) {
            return true;
        }
        visiting.push(name.to_string());
        let mut found = false;
        if let Some(e) = self.enums.get(name) {
            'outer: for v in &e.variants {
                for t in &v.payload {
                    if let Type::Named(n) = t {
                        if self.enums.contains_key(n) && self.enum_has_cycle(n, visiting) {
                            found = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        visiting.pop();
        found
    }

    fn is_copy(&self, t: &Type) -> bool {
        // Ссылка — это указатель: копирование не отнимает владение
        // у оригинала. Поэтому `println(r); println(r);` законно,
        // как `&T` в Rust (там `&T: Copy`).
        //
        // Enum — это тег: он ничего не владеет, поэтому копируется.
        match t {
            Type::Int | Type::Float | Type::Bool | Type::Str | Type::Ref(_) | Type::Slice(_) => true,
            Type::Named(n) => self.enums.contains_key(n),
            // `Option`/`Result` копируются, только если копируется
            // содержимое: иначе два владельца одной кучи.
            Type::Option(inner) => self.is_copy(inner),
            Type::Result(ok, err) => self.is_copy(ok) && self.is_copy(err),
            Type::Tuple(ts) => ts.iter().all(|t| self.is_copy(t)),
            _ => false,
        }
    }

    /// Enum с данными (включая `Option`/`Result`): в C это структура,
    /// поэтому `==` и печать оператором не поддерживаются.
    fn is_payload_enum_type(&self, t: &Type) -> bool {
        match t {
            Type::Option(_) | Type::Result(_, _) => true,
            Type::Named(n) => self
                .enums
                .get(n)
                .map(|en| en.variants.iter().any(|v| !v.payload.is_empty()))
                .unwrap_or(false),
            _ => false,
        }
    }

    fn types_compatible(&self, a: Type, b: &Type) -> bool {
        // `void` совместим только с `void`. Раньше здесь стояло
        // «если хоть одна сторона void — совместимо», и из-за этого
        // `return 5;` в void-функции проходил проверку, а потом падал
        // на сборке C: «return with a value, in function returning void».
        // Тот же безусловный пропуск был нужен, чтобы `void`-функцию
        // можно было вызвать как оператор — это проверяется отдельно,
        // в ветке вызова, и здесь не нужно.
        if a == Type::Void || b == &Type::Void {
            return a == Type::Void && b == &Type::Void;
        }
        if a.is_ref() || b.is_ref() {
            return a == *b;
        }
        match (&a, b) {
            (Type::Option(x), Type::Option(y)) => self.types_compatible((**x).clone(), y),
            (Type::Result(x1, x2), Type::Result(y1, y2)) => {
                self.types_compatible((**x1).clone(), y1)
                    && self.types_compatible((**x2).clone(), y2)
            }
            (Type::Vec(x), Type::Vec(y)) => self.types_compatible((**x).clone(), y),
            (Type::Slice(x), Type::Slice(y)) => self.types_compatible((**x).clone(), y),
            (Type::Slice(x), Type::Array(y, _)) => self.types_compatible((**x).clone(), y),
            _ => a == *b,
        }
    }

    fn dead_ref(&self, uid: u32) -> Option<&RefLocal> {
        self.refs.iter().find(|r| r.uid == uid && r.dead)
    }

    fn render(&self, p: &Path) -> String {
        // Корень пути — либо имя параметра (символический путь), либо uid
        // локальной переменной.
        let root = match p.root.parse::<u32>() {
            Ok(uid) => self.names.get(&uid).cloned().unwrap_or_else(|| p.root.clone()),
            Err(_) => p.root.clone(),
        };
        let mut s = root;
        for (i, seg) in p.segs.iter().enumerate() {
            match seg {
                Seg::Field(f) => {
                    if i == 0 {
                        s.push('.');
                    }
                    s.push_str(f);
                }
                Seg::Elems => s.push_str("[]"),
                Seg::Idx(i) => s.push_str(&format!("[{}]", i)),
            }
        }
        s
    }
}

enum RootKind {
    Owned(u32),
    ThroughRef,
}

/// Указывает ли путь на параметр функции (корень — имя, не uid).
///
/// Параметр живёт в кадре ВЫЗЫВАЮЩЕГО, поэтому всё, что мы запишем
/// по его пути, переживёт наш кадр. Локальная переменная — нет.
fn root_kind_is_param(p: &Path) -> bool {
    p.root.parse::<u32>().is_err()
}

/// Слияние состояний ветвей `if`. Ветвь, которая расходится
/// (`return`/`break`/`continue`), не влияет на код после `if`.
fn merge_if(
    base_scope: &HashMap<String, Local>,
    base_refs: &[RefLocal],
    then_scope: &HashMap<String, Local>,
    then_refs: &[RefLocal],
    then_diverged: bool,
    else_scope: &HashMap<String, Local>,
    else_refs: &[RefLocal],
    else_diverged: bool,
) -> (HashMap<String, Local>, Vec<RefLocal>, bool) {
    if then_diverged && else_diverged {
        return (base_scope.clone(), base_refs.to_vec(), true);
    }
    if then_diverged {
        return (else_scope.clone(), else_refs.to_vec(), false);
    }
    if else_diverged {
        return (then_scope.clone(), then_refs.to_vec(), false);
    }
    let mut scope = base_scope.clone();
    for (name, l) in scope.iter_mut() {
        if let Some(a) = then_scope.get(name).filter(|x| x.uid == l.uid) {
            l.moved = l.moved || a.moved;
            for e in &a.escapes {
                if !l.escapes.contains(e) {
                    l.escapes.push(e.clone());
                }
            }
        }
        if let Some(b) = else_scope.get(name).filter(|x| x.uid == l.uid) {
            l.moved = l.moved || b.moved;
            for e in &b.escapes {
                if !l.escapes.contains(e) {
                    l.escapes.push(e.clone());
                }
            }
        }
    }
    let mut refs = base_refs.to_vec();
    for r in refs.iter_mut() {
        if let Some(a) = then_refs.iter().find(|x| x.uid == r.uid) {
            if a.dead {
                r.dead = true;
                r.killed = a.killed.clone();
            }
        }
        if let Some(b) = else_refs.iter().find(|x| x.uid == r.uid) {
            if b.dead {
                r.dead = true;
                r.killed = b.killed.clone();
            }
        }
    }
    (scope, refs, false)
}

/// Слияние состояний, собранных в точках `break`/`continue`.
///
/// `break` уносит состояние на выход из цикла, `continue` — на начало
/// следующей итерации. Локальные переменные блока в состояние не входят:
/// сливаем только переменные с тем же uid, что уже есть в области.
fn merge_loop_states(
    scope: &mut HashMap<String, Local>,
    refs: &mut Vec<RefLocal>,
    states: &[(HashMap<String, Local>, Vec<RefLocal>)],
) {
    for (s, r) in states {
        for (name, l) in scope.iter_mut() {
            if let Some(a) = s.get(name).filter(|x| x.uid == l.uid) {
                l.moved = l.moved || a.moved;
                for e in &a.escapes {
                    if !l.escapes.contains(e) {
                        l.escapes.push(e.clone());
                    }
                }
            }
        }
        for rl in refs.iter_mut() {
            if let Some(a) = r.iter().find(|x| x.uid == rl.uid) {
                if a.dead {
                    rl.dead = true;
                    rl.killed = a.killed.clone();
                }
            }
        }
    }
}

/// Встречается ли имя `name` как идентификатор в выражении.
///
/// Нужно для точной диагностики: `result` в `post` функции с `void` —
/// не «неизвестная переменная», а «у этой функции нет возвращаемого
/// значения».
pub(crate) fn mentions_ident(e: &Expr, name: &str) -> bool {
    match &e.kind {
        ExprKind::Ident(n) => n == name,
        ExprKind::Field { obj, .. } => mentions_ident(obj, name),
        ExprKind::Index { obj, idx } => mentions_ident(obj, name) || mentions_ident(idx, name),
        ExprKind::Array(items) => items.iter().any(|i| mentions_ident(i, name)),
        ExprKind::Tuple(items) => items.iter().any(|i| mentions_ident(i, name)),
        ExprKind::Call { callee, args } => {
            mentions_ident(callee, name) || args.iter().any(|a| mentions_ident(a, name))
        }
        ExprKind::MethodCall { obj, args, .. } => {
            mentions_ident(obj, name) || args.iter().any(|a| mentions_ident(a, name))
        }
        ExprKind::Unary { e, .. } => mentions_ident(e, name),
        ExprKind::Binary { l, r, .. } => mentions_ident(l, name) || mentions_ident(r, name),
        ExprKind::Ref(inner) => mentions_ident(inner, name),
        ExprKind::If { cond, then_expr, else_expr } => {
            mentions_ident(cond, name)
                || mentions_ident(then_expr, name)
                || mentions_ident(else_expr, name)
        }
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Str(_) => false,
        ExprKind::StructInit { fields, .. } => {
            fields.iter().any(|(_, e)| mentions_ident(e, name))
        }
        ExprKind::Variant { .. } => false,
        ExprKind::Closure { .. } => false,
    }
}

#[derive(Default, Clone)]
struct LoopState {
    breaks: Vec<(HashMap<String, Local>, Vec<RefLocal>)>,
    continues: Vec<(HashMap<String, Local>, Vec<RefLocal>)>,
}

struct Ctx {
    scope: HashMap<String, Local>,
    /// Состояния, собранные в точках `break`/`continue` текущего цикла.
    loops: Vec<LoopState>,
    ret: Type,
    /// Пути, объявленные изменяемыми в этой функции.
    declared: Vec<Path>,
    /// Глубина вложенности циклов. `break` и `continue` допустимы
    /// только внутри цикла.
    loop_depth: usize,
    /// Это `main`? У main особый статус: его возврат — код выхода
    /// программы, и `return;` без значения означает «выход с нулём»
    /// (в C `main` тоже можно закончить без `return`).
    is_main: bool,
    /// Текущая ветвь завершается `return`/`break`/`continue`:
    /// код после неё недостижим, её состояние не сливается.
    diverged: bool,
}
