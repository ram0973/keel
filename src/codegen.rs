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
    out: String,
    /// Имя C-типа для каждой структуры Keel.
    ctype: HashMap<String, String>,
    /// Объявления структур — нужны для конструкторов и составных литералов.
    structs: HashMap<String, StructDecl>,
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
}

impl<'a> Codegen<'a> {
    pub fn new(prog: &'a Program, checker: &'a Checker) -> Codegen<'a> {
        let mut ctype = HashMap::new();
        let mut structs = HashMap::new();
        for item in &prog.items {
            if let Item::Struct(s) = item {
                ctype.insert(s.name.clone(), format!("Keel_{}", s.name));
                structs.insert(s.name.clone(), s.clone());
            }
        }
        Codegen {
            prog,
            types: &checker.expr_types,
            param_types: &checker.param_types,
            method_recv_ref: &checker.method_recv_ref,
            out: String::new(),
            ctype,
            structs,
            loop_depth: 0,
            loop_vars: HashMap::new(),
            in_struct_init: std::cell::Cell::new(false),
            shadow_count: std::cell::RefCell::new(HashMap::new()),
            var_names: std::cell::RefCell::new(HashMap::new()),
        }
    }

    fn ty(&self, t: &Type) -> String {
        match t {
            Type::Int => "int64_t".to_string(),
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
        }
    }

    /// Декларация переменной: `T name`, `T name[N]` или `T (*name)[N]`.
    fn decl(&self, t: &Type, name: &str) -> String {
        match t {
            Type::Array(elem, n) => format!("{} {}[{}]", self.ty(elem), name, n),
            // Ссылка — это указатель: `T* name`.
            Type::Ref(inner) => match inner.as_ref() {
                // Указатель на массив: скобки обязательны — `T (*name)[N]`.
                Type::Array(elem, n) => {
                    format!("{} (*{})[{}]", self.ty(elem), name, n)
                }
                t => format!("{}* {}", self.ty(t), name),
            },
            t => format!("{} {}", self.ty(t), name),
        }
    }

    pub fn generate(mut self) -> String {
        self.out.push_str("#include <stdint.h>\n#include <stdbool.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n");
        // Конкатенация строк: результат требует памяти, которой у операндов
        // нет. Копируем в свежий буфер из кучи.
        //
        // Память не освобождается: в Keel пока нет ни `free`, ни владения
        // для кучи. Для коротких программ это незаметно; настоящее
        // освобождение — в волне 8 (куча и растущие коллекции).
        self.out.push_str(
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

        for item in &self.prog.items {
            if let Item::Struct(s) = item {
                self.emit_struct(s);
            }
        }
        self.out.push('\n');
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
                let ret = match &f.ret {
                    Type::Void => "void".to_string(),
                    t => self.ty(t),
                };
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
                    params.push(self.decl(&pt, &p.name));
                }
                self.out.push_str(&format!(
                    "static {} {}({});\n",
                    ret,
                    self.fname_of(f),
                    params.join(", ")
                ));
            }
        }
        self.out.push('\n');

        for item in &self.prog.items {
            match item {
                Item::Func(f) => self.emit_func(f),
                Item::Impl(im) => {
                    for f in &im.funcs {
                        self.emit_func(f);
                    }
                }
                Item::Struct(_) => {}
                // Импорты обрабатывает загрузчик модулей: до
                // проверщика и генератора они не доходят.
                Item::Import(_) => {}
            }
        }
        self.out
    }

    fn emit_struct(&mut self, s: &StructDecl) {
        let cname = self.ctype.get(&s.name).cloned().unwrap_or_default();
        self.out.push_str(&format!("typedef struct {} {{\n", cname));
        for f in &s.fields {
            self.out.push_str(&format!("  {};\n", self.decl(&f.ty, &f.name)));
        }
        self.out.push_str(&format!("}} {};\n\n", cname));
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
        self.out.push_str(&text);
    }

    fn emit_func(&mut self, f: &FuncDecl) {
        // Имена переменных — свои в каждой функции.
        self.var_names.borrow_mut().clear();
        self.shadow_count.borrow_mut().clear();
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
            params.push(self.decl(&pt, &p.name));
            // Параметр занимает имя в C —登记, чтобы `let` с тем же
            // именем не переопределял его.
            self.shadow_count
                .borrow_mut()
                .insert(p.name.clone(), 1);
            self.var_names
                .borrow_mut()
                .insert(p.name.clone(), p.name.clone());
        }
        if f.name == "main" {
            self.out.push_str("int main(void) {\n");
        } else {
            let ret = match &f.ret {
                Type::Void => "void".to_string(),
                t => self.ty(t),
            };
            self.out.push_str(&format!(
                "static {} {}({}) {{\n",
                ret,
                self.fname_of(f),
                params.join(", ")
            ));
        }

        for stmt in &f.body.stmts {
            self.emit_stmt(stmt);
        }
        if f.name == "main" {
            self.out.push_str("  return 0;\n");
        }
        self.out.push_str("}\n\n");
    }

    fn emit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, ty, init, .. } => {
                let t = match ty {
                    Some(t) => t.clone(),
                    None => self.types.get(&init.id).cloned().unwrap_or(Type::Int),
                };
                // Затенение: второе `let x` получает другое имя в C.
                let name = &self.fresh_var_name(name);
                match &t {
                    Type::Array(elem, n) => {
                        let items = self.array_items(init);
                        self.out.push_str(&format!(
                            "  {} {}[{}] = {{{}}};\n",
                            self.ty(elem),
                            name,
                            n,
                            items.join(", ")
                        ));
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
                            self.out.push_str(&format!(
                                "  {} = {{{}}};\n",
                                self.decl(&t, name),
                                items.join(", ")
                            ));
                        } else {
                            self.out.push_str(&format!(
                                "  {} = {};\n",
                                self.decl(&t, name),
                                self.expr(init)
                            ));
                        }
                    }
                    t => {
                        self.out.push_str(&format!(
                            "  {} = {};\n",
                            self.decl(t, name),
                            self.expr(init)
                        ));
                    }
                }
            }
            Stmt::Assign { lhs, rhs } => {
                self.out.push_str(&format!("  {} = {};\n", self.expr(lhs), self.expr(rhs)));
            }
            Stmt::Expr(e) => {
                let s = self.expr(e);
                if s != "0" && !s.is_empty() {
                    self.out.push_str(&format!("  {};\n", s));
                }
            }
            Stmt::Return(e) => match e {
                Some(e) => self.out.push_str(&format!("  return {};\n", self.expr(e))),
                None => self.out.push_str("  return;\n"),
            },
            Stmt::If { cond, then_block, else_block } => {
                self.out
                    .push_str(&format!("  if ({}) {{\n", self.expr(cond)));
                for s in &then_block.stmts {
                    self.emit_stmt(s);
                }
                self.out.push_str("  }\n");
                if let Some(eb) = else_block {
                    self.out.push_str("  else {\n");
                    for s in &eb.stmts {
                        self.emit_stmt(s);
                    }
                    self.out.push_str("  }\n");
                }
            }
            Stmt::While { cond, body } => {
                self.out
                    .push_str(&format!("  while ({}) {{\n", self.expr(cond)));
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.out.push_str("  }\n");
            }
            Stmt::DoWhile { body, cond } => {
                self.out.push_str("  do {\n");
                for s in &body.stmts {
                    self.emit_stmt(s);
                }
                self.out
                    .push_str(&format!("  }} while ({});\n", self.expr(cond)));
            }
            Stmt::Break { .. } => {
                self.out.push_str("  break;\n");
            }
            Stmt::Continue { .. } => {
                self.out.push_str("  continue;\n");
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
                self.out.push_str(&format!(
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
                self.out.push_str("  }\n");
            }
            Stmt::ForEach { var, iter, body, .. } => {
                // Обход массива: индексируемся, копия элемента попадает
                // в переменную цикла.
                let arr = self.expr(iter);
                let (elem_ty, len) = self.array_info(iter);
                let (cvar, _) = self.loop_var(var);
                self.out.push_str(&format!(
                    "  for (int64_t __ki = 0; __ki < {}; __ki++) {{\n",
                    len
                ));
                self.out.push_str(&format!(
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
                self.out.push_str("  }\n");
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
    fn array_items(&self, init: &Expr) -> Vec<String> {
        match &init.kind {
            ExprKind::Array(items) => items.iter().map(|x| self.expr(x)).collect(),
            ExprKind::Call { args, .. } => args.iter().map(|x| self.expr(x)).collect(),
            _ => vec![],
        }
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
            ExprKind::Ident(name) => match self.loop_vars.get(name) {
                Some(cname) => cname.clone(),
                None => self.var_name(name),
            },
            ExprKind::Field { obj, name } => {
                let base = self.expr(obj);
                let base_ty = self.types.get(&obj.id);
                // Если base — ссылка (указатель), используем ->.
                match base_ty {
                    Some(Type::Ref(_)) => format!("{}->{}", base, name),
                    _ => format!("{}.{}", base, name),
                }
            }
            ExprKind::Index { obj, idx } => {
                let base = self.expr(obj);
                let base_ty = self.types.get(&obj.id);
                match base_ty {
                    // Ссылка на массив (T (*)[N]): индексация в C дала бы
                    // сам массив, а не элемент — нужно разыменовать.
                    Some(Type::Ref(inner)) if matches!(inner.as_ref(), Type::Array(..)) => {
                        format!("(*{})[{}]", base, self.expr(idx))
                    }
                    _ => format!("{}[{}]", base, self.expr(idx)),
                }
            }
            ExprKind::Call { callee, args } => {
                let ExprKind::Ident(name) = &callee.kind else {
                    return "0".to_string();
                };
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
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("{}({})", self.fname(name), a.join(", "))
            }
            ExprKind::MethodCall { obj, name, args } => {
                // Метод вызывается как `k_Тип_метод(получатель, args...)`.
                let obj_ty = self.types.get(&obj.id).map(|t| t.deref().clone());
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
                let elem_ty = match self.types.get(&e.id) {
                    Some(Type::Array(t, _)) => self.ty(t),
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
