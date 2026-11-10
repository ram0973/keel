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
    out: String,
    /// Имя C-типа для каждой структуры Keel.
    ctype: HashMap<String, String>,
    /// Объявления структур — нужны для конструкторов и составных литералов.
    structs: HashMap<String, StructDecl>,
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
            out: String::new(),
            ctype,
            structs,
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

        for item in &self.prog.items {
            if let Item::Struct(s) = item {
                self.emit_struct(s);
            }
        }
        self.out.push('\n');
        // Прототипы: в C функция должна быть объявлена до вызова.
        for item in &self.prog.items {
            if let Item::Func(f) = item {
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
                                .get(&(f.name.clone(), p.name.clone()))
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
                    self.fname(&f.name),
                    params.join(", ")
                ));
            }
        }
        self.out.push('\n');

        for item in &self.prog.items {
            if let Item::Func(f) = item {
                self.emit_func(f);
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

    fn emit_func(&mut self, f: &FuncDecl) {
        let mut params = Vec::new();
        for p in &f.params {
            let pt = match &p.kind {
                ParamKind::Owned => p.ty.clone(),
                ParamKind::Ref => Type::Ref(Box::new(p.ty.clone())),
                // Тип параметра-внутри выведен проверщиком.
                ParamKind::RefIn(_) => {
                    let inner = self
                        .param_types
                        .get(&(f.name.clone(), p.name.clone()))
                        .cloned()
                        .unwrap_or_else(|| p.ty.clone());
                    Type::Ref(Box::new(inner))
                }
            };
            params.push(self.decl(&pt, &p.name));
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
                self.fname(&f.name),
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
        }
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
        for (i, f) in s.fields.iter().enumerate() {
            let v = match args.get(i) {
                Some(a) => self.expr(a),
                None => "0".to_string(),
            };
            out.push(format!(".{} = {}", f.name, v));
        }
        out
    }

    /// Имя функции в C. Пользовательские имена получают префикс `k_`,
    /// чтобы не сталкиваться со стандартной библиотекой (`remove` из
    /// stdio.h, `index` из strings.h и т.д.). `main` остаётся как есть —
    /// это точка входа C.
    fn fname(&self, name: &str) -> String {
        if name == "main" {
            name.to_string()
        } else {
            format!("k_{}", name)
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
            ExprKind::Ident(name) => name.clone(),
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
                if name == "print" {
                    if let Some(a) = args.first() {
                        let t = self.types.get(&a.id).cloned().unwrap_or(Type::Int);
                        let mut v = self.expr(a);
                        // Ссылка печатается по значению: авторазыменование.
                        let mut t = t;
                        if let Type::Ref(inner) = t.clone() {
                            v = format!("(*{})", v);
                            t = *inner;
                        }
                        return match t {
                            Type::Str => format!("printf(\"%s\\n\", {})", v),
                            Type::Bool => {
                                format!("printf(\"%s\\n\", {} ? \"true\" : \"false\")", v)
                            }
                            _ => format!("printf(\"%lld\\n\", (long long){})", v),
                        };
                    }
                    return String::new();
                }
                // Конструктор структуры → составной литерал.
                if let Some(s) = self.structs.get(name) {
                    let cname = self.ctype.get(name).cloned().unwrap_or_else(|| name.clone());
                    let mut items = Vec::new();
                    for (i, f) in s.fields.iter().enumerate() {
                        let v = match args.get(i) {
                            Some(a) => self.expr(a),
                            None => "0".to_string(),
                        };
                        items.push(format!(".{} = {}", f.name, v));
                    }
                    return format!("({}){{{}}}", cname, items.join(", "));
                }
                let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect();
                format!("{}({})", self.fname(name), a.join(", "))
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
            ExprKind::Array(items) => {
                let elems: Vec<String> = items.iter().map(|x| self.expr(x)).collect();
                format!("{{{}}}", elems.join(", "))
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
