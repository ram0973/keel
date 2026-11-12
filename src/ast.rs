//! AST языка Keel (подмножество v0 для проверки идей).

use std::fmt;

// ============================================================
// Типы
// ============================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    Str,
    Void,
    Named(String),
    Ref(Box<Type>),
    Array(Box<Type>, usize),
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
            Type::Bool => "bool".into(),
            Type::Str => "string".into(),
            Type::Void => "void".into(),
            Type::Named(n) => n.clone(),
            Type::Ref(t) => format!("&{}", t.name()),
            Type::Array(t, n) => format!("[{}; {}]", t.name(), n),
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
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: String,
    pub fields: Vec<Field>,
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
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum Item {
    Struct(StructDecl),
    Func(FuncDecl),
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
    Bool(bool),
    Str(String),
    Ident(String),
    Field { obj: Box<Expr>, name: String },
    Index { obj: Box<Expr>, idx: Box<Expr> },
    /// Массив-литерал: `[a, b, c]`.
    Array(Vec<Expr>),
    Call { callee: Box<Expr>, args: Vec<Expr> },
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
    Return(Option<Expr>),
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
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
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}
