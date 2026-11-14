//! AST языка Keel (подмножество v0 для проверки идей).

use std::fmt;

// ============================================================
// Типы
// ============================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    Str,
    Void,
    Named(String),
    Ref(Box<Type>),
    Array(Box<Type>, usize),
    /// Встроенный `Option<T>`: `None` или `Some(T)`.
    Option(Box<Type>),
    /// Встроенный `Result<T, E>`: `Ok(T)` или `Err(E)`.
    Result(Box<Type>, Box<Type>),
    /// Встроенный `Vec<T>`: растущий массив в куче.
    Vec(Box<Type>),
    /// Кортеж `(T, U)`.
    Tuple(Vec<Type>),
    /// Срез `[T]`: указатель на элементы + длина.
    Slice(Box<Type>),
    /// Функция `fn(T, U) -> V`.
    Fn(Vec<Type>, Box<Type>),
}

impl Type {
    pub fn deref(&self) -> &Type {
        match self {
            Type::Ref(t) => t,
            t => t,
        }
    }

    pub fn is_ref(&self) -> bool {
        matches!(self, Type::Ref(_))
    }

    /// Имя типа для сообщений об ошибках.
    pub fn name(&self) -> String {
        match self {
            Type::Int => "int".into(),
            Type::Float => "float".into(),
            Type::Bool => "bool".into(),
            Type::Str => "string".into(),
            Type::Void => "void".into(),
            Type::Named(n) => n.clone(),
            Type::Ref(t) => format!("&{}", t.name()),
            Type::Array(t, n) => format!("[{}; {}]", t.name(), n),
            Type::Option(t) => format!("Option<{}>", t.name()),
            Type::Result(t, e) => format!("Result<{}, {}>", t.name(), e.name()),
            Type::Vec(t) => format!("Vec<{}>", t.name()),
            Type::Tuple(ts) => format!(
                "({})",
                ts.iter().map(|t| t.name()).collect::<Vec<_>>().join(", ")
            ),
            Type::Slice(t) => format!("[{}]", t.name()),
            Type::Fn(args, ret) => format!(
                "fn({}) -> {}",
                args.iter().map(|t| t.name()).collect::<Vec<_>>().join(", "),
                ret.name()
            ),
        }
    }

    /// Имя типа в C для мономорфизации встроенных обобщённых типов.
    pub fn mangle(&self) -> String {
        match self {
            Type::Int => "int".into(),
            Type::Float => "float".into(),
            Type::Bool => "bool".into(),
            Type::Str => "string".into(),
            Type::Void => "void".into(),
            Type::Named(n) => n.clone(),
            Type::Ref(t) => format!("ref_{}", t.mangle()),
            Type::Array(t, n) => format!("arr{}_{}", n, t.mangle()),
            Type::Option(t) => format!("Option_{}", t.mangle()),
            Type::Result(t, e) => format!("Result_{}_{}", t.mangle(), e.mangle()),
            Type::Vec(t) => format!("Vec_{}", t.mangle()),
            Type::Tuple(ts) => format!(
                "Tuple_{}",
                ts.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_")
            ),
            Type::Slice(t) => format!("Slice_{}", t.mangle()),
            Type::Fn(args, ret) => format!(
                "Fn_{}_{}",
                args.iter().map(|t| t.mangle()).collect::<Vec<_>>().join("_"),
                ret.mangle()
            ),
        }
    }
}

// ============================================================
// Пути (path) — основа group borrowing
// ============================================================

/// Сегмент пути: поле структуры, «элементы коллекции» или конкретный индекс.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Seg {
    Field(String),
    Elems,
    Idx(i64),
}

/// Путь до объекта: корень (локальная переменная или параметр) + сегменты.
/// `world.entities[]` = root "world", segs [Field("entities"), Elems].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    pub root: String,
    pub segs: Vec<Seg>,
}

impl Path {
    pub fn root(name: &str) -> Path {
        Path { root: name.to_string(), segs: Vec::new() }
    }

    pub fn child(&self, seg: Seg) -> Path {
        let mut segs = self.segs.clone();
        segs.push(seg);
        Path { root: self.root.clone(), segs }
    }

    /// `self` — префикс `other` (включая равенство)?
    ///
    /// Сегмент `Elems` («элементы коллекции») — шаблон: он совпадает с
    /// любым конкретным индексом. Поэтому `world.entities[]` является
    /// префиксом `world.entities[0].hp` — как и задумано в Valen.
    pub fn is_prefix_of(&self, other: &Path) -> bool {
        if self.root != other.root {
            return false;
        }
        if other.segs.len() < self.segs.len() {
            return false;
        }
        self.segs.iter().zip(other.segs.iter()).all(|(a, b)| match (a, b) {
            (Seg::Elems, Seg::Idx(_)) => true,
            (Seg::Elems, Seg::Elems) => true,
            (a, b) => a == b,
        })
    }

    /// `other` — строго внутри `self`?
    pub fn strictly_inside(&self, other: &Path) -> bool {
        self.is_prefix_of(other) && self.segs.len() < other.segs.len()
    }

    /// Могут ли пути указывать на один и тот же объект — хотя бы по
    /// одному элементу коллекции?
    ///
    /// Путь `a[]` — это группа элементов (индекс неизвестен), а `a[0]` —
    /// конкретный элемент. Они могут указывать на один объект: группа
    /// содержит все элементы сразу. Поэтому запись в `a[0]` затрагивает
    /// ссылку на `a[]` — она могла указывать именно на `a[0]`.
    ///
    /// Используется при инвалидации: ссылка на элемент с динамическим
    /// индексом хранит путь-группу, и любая запись в элемент этой
    /// коллекции её гасит.
    pub fn overlaps(&self, other: &Path) -> bool {
        if self.root != other.root {
            return false;
        }
        if self.segs.len() != other.segs.len() {
            return false;
        }
        let mut cross = false;
        for (a, b) in self.segs.iter().zip(other.segs.iter()) {
            match (a, b) {
                // Группа и конкретный элемент — один и тот же объект.
                (Seg::Elems, Seg::Idx(_)) | (Seg::Idx(_), Seg::Elems) => cross = true,
                (Seg::Elems, Seg::Elems) => {}
                (x, y) => {
                    if x != y {
                        return false;
                    }
                }
            }
        }
        cross
    }

    pub fn render(&self) -> String {
        let mut s = self.root.clone();
        for seg in &self.segs {
            match seg {
                Seg::Field(f) => s.push_str(&format!(".{}", f)),
                Seg::Elems => s.push_str("[]"),
                Seg::Idx(i) => s.push_str(&format!("[{}]", i)),
            }
        }
        s
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

// ============================================================
// Объявления
// ============================================================

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    /// Строка объявления поля — для диагностик.
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
    /// Строка объявления — для диагностик о повторных именах.
    pub line: usize,
    /// Файл объявления — для диагностик. Пусто при `compile(src)`.
    pub file: String,
}

/// `enum Name { A, B, C }` — именованный набор состояний.
///
/// Варианты могут нести позиционные данные: `Some(int)`. Данные пока
/// ограничены копируемыми типами (int, bool, string, enum), чтобы
/// деструктуризация в `match` не двигала значение из enum.
#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<VariantDecl>,
    /// Строка объявления — для диагностик о повторных именах.
    pub line: usize,
    /// Файл объявления — для диагностик. Пусто при `compile(src)`.
    pub file: String,
}

/// Вариант enum: имя и позиционные данные.
#[derive(Clone, Debug)]
pub struct VariantDecl {
    pub name: String,
    pub payload: Vec<Type>,
    pub line: usize,
}

/// Как объявлен параметр.
#[derive(Clone, Debug)]
pub enum ParamKind {
    /// Обычный параметр, владение переносится в функцию.
    Owned,
    /// `x &T` — ссылка на объект целиком.
    Ref,
    /// `x in path` — ссылка на элемент внутри `path`.
    RefIn(Path),
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub kind: ParamKind,
    pub mutable: bool,
}

/// Что функция объявляет изменяемым.
/// `param` — имя параметра (корень пути), `segs` — путь внутрь него.
#[derive(Clone, Debug)]
pub struct MutPath {
    pub param: String,
    pub segs: Vec<Seg>,
}

#[derive(Clone, Debug)]
pub struct FuncDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Type,
    pub mut_paths: Vec<MutPath>,
    /// Предусловия `pre(...)`: проверяются на входе в функцию.
    pub pre: Vec<Expr>,
    /// Постусловия `post(...)`: проверяются перед каждым `return`.
    /// В выражениях доступно имя `result` — возвращаемое значение.
    pub post: Vec<Expr>,
    pub body: Block,
    /// Для функции из `impl`-блока — имя типа-владельца. У свободной
    /// функции `None`. Нужно, чтобы методы разных типов с одинаковым
    /// именем не сталкивались.
    pub owner: Option<String>,
    /// Строка объявления — для диагностик о повторных именах.
    pub line: usize,
    /// Файл объявления — для диагностик. Пусто при `compile(src)`.
    pub file: String,
}

#[derive(Clone, Debug)]
pub enum Item {
    Struct(StructDecl),
    Enum(EnumDecl),
    Func(FuncDecl),
    Impl(ImplDecl),
    ///  — подключить другой файл.
    ///
    /// Имена из подключённого файла становятся видны как свои:
    /// пространство имён плоское. Путь — относительно каталога
    /// импортирующего файла.
    Import(ImportDecl),
}

#[derive(Clone, Debug)]
pub struct ImportDecl {
    pub path: String,
    pub line: usize,
}

/// `impl Type { ... }` — методы и ассоциированные функции типа.
///
/// Метод отличается от ассоциированной функции тем, что его первый
/// параметр называется `self`. `self` — обычный параметр: он может быть
/// ссылкой (`self &T`) или значением (`self T`), и на него действуют те
/// же правила владения, что и на любой другой параметр.
#[derive(Clone, Debug)]
pub struct ImplDecl {
    /// Имя типа, для которого объявлен блок.
    pub ty: String,
    pub funcs: Vec<FuncDecl>,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub items: Vec<Item>,
}

// ============================================================
// Выражения и операторы
// ============================================================

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Ident(String),
    Field { obj: Box<Expr>, name: String },
    Index { obj: Box<Expr>, idx: Box<Expr> },
    /// Массив-литерал: `[a, b, c]`.
    Array(Vec<Expr>),
    /// Кортеж-литерал: `(a, b)`.
    Tuple(Vec<Expr>),
    Call { callee: Box<Expr>, args: Vec<Expr> },
    /// `Type::Variant` — значение варианта enum.
    Variant { ty: String, name: String },
    /// `Point { x: 3, y: 4 }` — именованный конструктор структуры.
    StructInit { name: String, fields: Vec<(String, Expr)> },
    /// `x.method(args)` — вызов метода. Отличается от `Field` наличием
    /// скобок; разрешается в обычный вызов с получателем первым
    /// аргументом.
    MethodCall {
        obj: Box<Expr>,
        name: String,
        args: Vec<Expr>,
    },
    Unary { op: UnOp, e: Box<Expr> },
    Binary { op: BinOp, l: Box<Expr>, r: Box<Expr> },
    Ref(Box<Expr>),
    /// `if cond { a } else { b }` как выражение. Ветви — одиночные
    /// выражения; значение — результат выбранной ветви.
    If {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// Замыкание `|x| expr` или `|x| { ... }`.
    Closure {
        params: Vec<Param>,
        body: Block,
    },
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: u32,
    pub line: usize,
    pub kind: ExprKind,
}

#[derive(Clone, Copy, Debug)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let {
        name: String,
        mutable: bool,
        ty: Option<Type>,
        init: Expr,
    },
    Assign {
        lhs: Expr,
        rhs: Expr,
    },
    Expr(Expr),
    /// `return expr;` или `return;`. Номер строки нужен для диагностики:
    /// `return;` в функции, возвращающей значение, — ошибка.
    Return {
        line: usize,
        value: Option<Expr>,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `do { ... } while cond;` — тело выполняется хотя бы один раз.
    DoWhile {
        body: Block,
        cond: Expr,
    },
    /// `break` — выход из ближайшего цикла.
    Break {
        line: usize,
    },
    /// `continue` — к следующей итерации ближайшего цикла.
    Continue {
        line: usize,
    },
    /// `for x in a..b` — цикл по диапазону целых чисел.
    /// Правая граница исключительная, как в Rust и Zeta: `0..3` — это
    /// 0, 1, 2.
    ForRange {
        line: usize,
        var: String,
        start: Expr,
        end: Expr,
        body: Block,
    },
    /// `for x in arr` — цикл по элементам массива.
    ForEach {
        line: usize,
        var: String,
        iter: Expr,
        body: Block,
    },
    /// `match expr { Type::Variant => { ... } ... }` — разбор по тегу.
    Match {
        line: usize,
        subject: Expr,
        arms: Vec<MatchArm>,
    },
}

/// Ветвь `match`: `Enum::Вариант => { ... }` или `Enum::Вариант(x) => { ... }`.
#[derive(Clone, Debug)]
pub struct MatchArm {
    pub ty: String,
    pub variant: String,
    /// Имена, привязанные к данным варианта: `Some(x)` → `["x"]`.
    pub bindings: Vec<String>,
    pub body: Block,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}
