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

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub line: usize,
    pub msg: String,
}

impl Diagnostic {
    fn new(line: usize, msg: String) -> Self {
        Diagnostic { line, msg }
    }
}

// ============================================================
// Локальные переменные
// ============================================================

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
}

#[derive(Clone)]
struct RefLocal {
    uid: u32,
    target: Path,
    dead: bool,
}

// ============================================================
// Проверщик
// ============================================================

pub struct Checker {
    structs: HashMap<String, StructDecl>,
    funcs: HashMap<String, FuncInfo>,
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
    diags: Vec<Diagnostic>,
}

#[derive(Clone)]
struct FuncInfo {
    params: Vec<Param>,
    ret: Type,
    mut_paths: Vec<MutPath>,
}

impl Checker {
    pub fn new() -> Checker {
        Checker {
            structs: HashMap::new(),
            funcs: HashMap::new(),
            names: HashMap::new(),
            next_uid: 1,
            refs: Vec::new(),
            modified: Vec::new(),
            expr_types: HashMap::new(),
            param_types: HashMap::new(),
            diags: Vec::new(),
        }
    }

    pub fn check(&mut self, program: &Program) -> Vec<Diagnostic> {
        // Сначала собираем объявления, чтобы функции могли ссылаться друг на друга.
        for item in &program.items {
            match item {
                Item::Struct(s) => {
                    self.structs.insert(s.name.clone(), s.clone());
                }
                Item::Func(f) => {
                    self.funcs.insert(
                        f.name.clone(),
                        FuncInfo {
                            params: f.params.clone(),
                            ret: f.ret.clone(),
                            mut_paths: f.mut_paths.clone(),
                        },
                    );
                }
            }
        }

        // Затем проверяем тела.
        for item in &program.items {
            if let Item::Func(f) = item {
                self.check_func(f);
            }
        }

        self.diags.clone()
    }

    /// Имена параметров функции — только их пути требуют mut-контракта.
    fn param_names(f: &FuncDecl) -> Vec<String> {
        f.params.iter().map(|p| p.name.clone()).collect()
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
                        .insert((f.name.clone(), p.name.clone()), inner.clone());
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
            };

            // Путь параметра-ссылки должен начинаться с его же имени либо
            // с имени другого параметра (например `x in world.entities[]`).
            if let ParamKind::RefIn(path) = &p.kind {
                local.path = Path::root(&p.name);
                let _ = path;
            }

            if let Some(t) = &local.ref_target {
                local.ref_uid = Some(uid);
                self.refs.push(RefLocal { uid, target: t.clone(), dead: false });
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
        };
        self.check_block(&f.body, &mut ctx);

        // Проверка mut-контракта: тело не должно менять то, что не объявлено.
        // Локальные переменные — собственность функции, их объявлять не нужно;
        // контракт касается только путей, уходящих в параметры.
        let params = Self::param_names(f);
        for (path, _is_replace) in self.modified.clone() {
            // Пропускаем изменения локальных переменных.
            if !params.contains(&path.root) {
                continue;
            }
            if !declared.iter().any(|d| d.is_prefix_of(&path)) {
                self.diags.push(Diagnostic::new(
                    0,
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

    fn check_block(&mut self, b: &Block, ctx: &mut Ctx) {
        for stmt in &b.stmts {
            self.check_stmt(stmt, ctx);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt, ctx: &mut Ctx) {
        match stmt {
            Stmt::Let { name, mutable, ty, init } => {
                let mut init_ty = self.type_of(init, ctx);
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
                if !init_ty.is_ref() {
                    self.record_move(init, ctx);
                }
                let uid = self.next_uid;
                self.next_uid += 1;
                self.names.insert(uid, name.clone());

                // Если инициализатор — ссылка, запоминаем её цель.
                let ref_target = if init_ty.is_ref() {
                    self.place_path_of(init, ctx)
                } else {
                    None
                };
                let ref_uid = ref_target.as_ref().map(|t| {
                    self.refs.push(RefLocal { uid, target: t.clone(), dead: false });
                    uid
                });

                // Ссылка на локальную переменную: её путь — путь самой
                // переменной, а не новый корень. Иначе инвалидация
                // не сработала бы.
                let own_path = if init_ty.is_ref() {
                    ref_target.clone().unwrap_or_else(|| Path::root(&uid.to_string()))
                } else {
                    Path::root(&uid.to_string())
                };

                ctx.scope.insert(
                    name.clone(),
                    Local {
                        uid,
                        ty: ty.clone().unwrap_or(init_ty),
                        path: own_path,
                        ref_target,
                        ref_uid,
                        moved: false,
                        is_mut: *mutable,
                    },
                );
            }
            Stmt::Assign { lhs, rhs } => {
                self.check_assign(lhs, rhs, ctx);
            }
            Stmt::Expr(e) => {
                self.type_of(e, ctx);
            }
            Stmt::Return(e) => {
                if let Some(e) = e {
                    let t = self.type_of(e, ctx);
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
                    self.record_move(e, ctx);
                }
            }
            Stmt::If { cond, then_block, else_block } => {
                self.type_of(cond, ctx);
                self.check_block(then_block, ctx);
                if let Some(eb) = else_block {
                    self.check_block(eb, ctx);
                }
            }
            Stmt::While { cond, body } => {
                self.type_of(cond, ctx);
                // Тело проверяем дважды: второй проход стартует с состоянием
                // после первого, поэтому ловит использование ссылки,
                // инвалидированной в предыдущей итерации.
                self.check_block(body, ctx);
                self.check_block(body, ctx);
            }
        }
    }

    fn check_assign(&mut self, lhs: &Expr, rhs: &Expr, ctx: &mut Ctx) {
        let (path, root_kind) = self.resolve_place(lhs, ctx);
        let lhs_ty = self.type_of(lhs, ctx);
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

        self.record_move(rhs, ctx);

        // Присваивание ЗАМЕНЯЕТ значение по пути.
        self.modified.push((path.clone(), true));
        self.invalidate_replace(&path);
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
    fn invalidate_replace(&mut self, path: &Path) {
        for r in self.refs.iter_mut() {
            if path.is_prefix_of(&r.target) {
                r.dead = true;
            }
        }
    }

    /// Инвалидирует ссылки строго внутри пути (содержимое изменено).
    fn invalidate_inside(&mut self, path: &Path) {
        for r in self.refs.iter_mut() {
            if path.is_prefix_of(&r.target) && r.target != *path {
                r.dead = true;
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

    fn type_of_inner(&mut self, e: &Expr, ctx: &mut Ctx) -> Type {
        match &e.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Str(_) => Type::Str,
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
                    if self.ref_dead(uid) {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!(
                                "ссылка `{}` инвалидирована: объект `{}` был изменён или уничтожен",
                                name,
                                self.render(local.ref_target.as_ref().unwrap_or(&Path::root("?")))
                            ),
                        ));
                    }
                }
                local.ty
            }
            ExprKind::Field { obj, name } => {
                let base = self.type_of(obj, ctx);
                let base = base.deref().clone();
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
                    _ => {
                        self.diags.push(Diagnostic::new(
                            e.line,
                            format!("`{}` — не структура, поле недоступно", base.name()),
                        ));
                        Type::Void
                    }
                }
            }
            ExprKind::Index { obj, idx } => {
                let base = self.type_of(obj, ctx);
                self.type_of(idx, ctx);
                let base = base.deref().clone();
                match base {
                    Type::Array(elem, _) => *elem,
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
                    let t = self.type_of(it, ctx);
                    if i == 0 {
                        elem = t;
                    }
                    self.record_move(it, ctx);
                }
                Type::Array(Box::new(elem), items.len())
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
                    UnOp::Not => Type::Bool,
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
                        Type::Bool
                    }
                    BinOp::And | BinOp::Or => Type::Bool,
                    _ => {
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
        }
    }

    fn type_of_call(&mut self, callee: &Expr, args: &[Expr], ctx: &mut Ctx, line: usize) -> Type {
        let ExprKind::Ident(name) = &callee.kind else {
            self.diags.push(Diagnostic::new(line, "вызов: поддерживается только имя функции".into()));
            return Type::Void;
        };

        // Конструктор структуры?
        if let Some(s) = self.structs.get(name).cloned() {
            if args.len() != s.fields.len() {
                self.diags.push(Diagnostic::new(
                    line,
                    format!(
                        "конструктор `{}` ждёт {} аргументов, передан {}",
                        name,
                        s.fields.len(),
                        args.len()
                    ),
                ));
            }
            for (a, f) in args.iter().zip(s.fields.iter()) {
                let at = self.type_of(a, ctx);
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
                self.record_move(a, ctx);
            }
            return Type::Named(name.clone());
        }

        // Встроенная функция?
        if name == "print" {
            for a in args {
                self.type_of(a, ctx);
                self.record_move(a, ctx);
            }
            return Type::Void;
        }

        let Some(fi) = self.funcs.get(name).cloned() else {
            self.diags.push(Diagnostic::new(line, format!("неизвестная функция `{}`", name)));
            return Type::Void;
        };

        if args.len() != fi.params.len() {
            self.diags.push(Diagnostic::new(
                line,
                format!(
                    "`{}` ждёт {} аргументов, передан {}",
                    name,
                    fi.params.len(),
                    args.len()
                ),
            ));
        }

        // Для каждого аргумента вычисляем его путь — он нужен, чтобы
        // перенести mut-контракт вызываемой функции на вызывающего.
        let mut arg_paths: Vec<Option<Path>> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let t = self.type_of(a, ctx);
            // Путь есть только у «мест» (переменных и их полей).
            let p = self.place_path_of(a, ctx);
            arg_paths.push(p);

            let Some(param) = fi.params.get(i) else { continue };
            match param.kind {
                ParamKind::Owned => {
                    // Передача владения — перемещение.
                    if !self.is_copy(&t) {
                        self.record_move(a, ctx);
                    }
                }
                ParamKind::Ref | ParamKind::RefIn(_) => {
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
            self.modified.push((effective.clone(), false));
            self.invalidate_inside(&effective);
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
            self.modified.push((effective.clone(), false));
            self.invalidate_inside(&effective);
        }

        fi.ret.clone()
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

    /// Помечает источник перемещённым, если он не Copy.
    fn record_move(&mut self, e: &Expr, ctx: &mut Ctx) {
        let ExprKind::Ident(name) = &e.kind else { return };
        let Some(local) = ctx.scope.get(name).cloned() else { return };
        if self.is_copy(&local.ty) {
            return;
        }
        if let Some(l) = ctx.scope.get_mut(name) {
            l.moved = true;
        }
    }

    fn is_copy(&self, t: &Type) -> bool {
        matches!(t, Type::Int | Type::Bool | Type::Str)
    }

    fn types_compatible(&self, a: Type, b: &Type) -> bool {
        if a == Type::Void || b == &Type::Void {
            return true;
        }
        if a.is_ref() || b.is_ref() {
            return a == *b;
        }
        a == *b
    }

    fn ref_dead(&self, uid: u32) -> bool {
        self.refs.iter().any(|r| r.uid == uid && r.dead)
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

struct Ctx {
    scope: HashMap<String, Local>,
    ret: Type,
    /// Пути, объявленные изменяемыми в этой функции.
    declared: Vec<Path>,
}
