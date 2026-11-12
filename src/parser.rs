//! Парсер Keel (рекурсивный спуск).

use crate::ast::*;
use crate::lexer::{lex, Tok, Token};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    next_id: u32,
}

type PResult<T> = Result<T, String>;

impl Parser {
    pub fn new(src: &str) -> PResult<Parser> {
        Ok(Parser { toks: lex(src)?, pos: 0, next_id: 0 })
    }

    /// Парсер с заданным начальным значением счётчика id.
    ///
    /// Нужно при загрузке нескольких файлов: id выражений должны быть
    /// уникальны **по всей программе**, иначе тип выражения из одного
    /// файла попадёт на выражение из другого — и сгенерированный C
    /// окажется неверным (`self.name` вместо `self->name`).
    pub fn with_id_start(src: &str, start: u32) -> PResult<Parser> {
        Ok(Parser { toks: lex(src)?, pos: 0, next_id: start })
    }

    /// Сколько id израсходовано — чтобы продолжить нумерацию в
    /// следующем файле.
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    // --- вспомогательное ---

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }

    fn line(&self) -> usize {
        self.toks[self.pos].line
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(s) if s == p)
    }

    fn at_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == k)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> PResult<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(format!(
                "строка {}: ожидалось '{}', получено {}",
                self.line(),
                p,
                self.describe(self.peek())
            ))
        }
    }

    fn expect_kw(&mut self, k: &str) -> PResult<()> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            Err(format!(
                "строка {}: ожидалось '{}', получено {}",
                self.line(),
                k,
                self.describe(self.peek())
            ))
        }
    }

    fn describe(&self, t: &Tok) -> String {
        match t {
            Tok::Int(n) => format!("число {}", n),
            Tok::Ident(s) => format!("'{}'", s),
            Tok::Str(_) => "строка".to_string(),
            Tok::Punct(s) => format!("'{}'", s),
            Tok::Eof => "конец файла".to_string(),
        }
    }

    fn ident(&mut self) -> PResult<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(format!(
                "строка {}: ожидалось имя, получено {}",
                self.line(),
                self.describe(&other)
            )),
        }
    }

    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn mk(&mut self, line: usize, kind: ExprKind) -> Expr {
        Expr { id: self.id(), line, kind }
    }

    // --- грамматика ---

    pub fn parse_program(&mut self) -> PResult<Program> {
        let mut items = Vec::new();
        while !matches!(self.peek(), Tok::Eof) {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> PResult<Item> {
        if self.at_kw("struct") {
            Ok(Item::Struct(self.parse_struct()?))
        } else if self.at_kw("fn") {
            Ok(Item::Func(self.parse_func()?))
        } else if self.at_kw("impl") {
            Ok(Item::Impl(self.parse_impl()?))
        } else if self.at_kw("import") {
            Ok(Item::Import(self.parse_import()?))
        } else {
            Err(format!(
                "строка {}: ожидалось 'struct', 'fn', 'impl' или 'import', получено {}",
                self.line(),
                self.describe(self.peek())
            ))
        }
    }

    /// 
    fn parse_import(&mut self) -> PResult<ImportDecl> {
        let line = self.line();
        self.expect_kw("import")?;
        let path = match self.bump() {
            Tok::Str(s) => s,
            other => {
                return Err(format!(
                    "строка {}: после  ожидался путь в кавычках, получено {}",
                    self.line(),
                    self.describe(&other)
                ))
            }
        };
        self.expect_punct(";")?;
        Ok(ImportDecl { path, line })
    }

    /// `impl Type { fn ... }` — блок методов и ассоциированных функций.
    fn parse_impl(&mut self) -> PResult<ImplDecl> {
        self.expect_kw("impl")?;
        let ty = self.ident()?;
        self.expect_punct("{")?;
        let mut funcs = Vec::new();
        while !self.at_punct("}") {
            self.expect_kw("fn")?;
            let name = self.ident()?;
            funcs.push(self.parse_func_rest(name, Some(ty.clone()))?);
        }
        self.expect_punct("}")?;
        Ok(ImplDecl { ty, funcs })
    }

    fn parse_struct(&mut self) -> PResult<StructDecl> {
        let line = self.line();
        self.expect_kw("struct")?;
        let name = self.ident()?;
        self.expect_punct("{")?;
        let mut fields = Vec::new();
        while !self.at_punct("}") {
            let fline = self.line();
            let fname = self.ident()?;
            let ty = self.parse_type()?;
            self.expect_punct(";")?;
            fields.push(Field { name: fname, ty, line: fline });
        }
        self.expect_punct("}")?;
        Ok(StructDecl { name, fields, line })
    }

    fn parse_func(&mut self) -> PResult<FuncDecl> {
        self.expect_kw("fn")?;
        let name = self.ident()?;
        self.parse_func_rest(name, None)
    }

    /// Общая часть после имени: параметры, тип возврата, контракт, тело.
    /// `owner` — имя типа, если функция объявлена в `impl`-блоке.
    fn parse_func_rest(&mut self, name: String, owner: Option<String>) -> PResult<FuncDecl> {
        let line = self.line();
        self.expect_punct("(")?;
        let mut params = Vec::new();
        while !self.at_punct(")") {
            params.push(self.parse_param()?);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;

        // Возвращаемый тип. Если дальше `mut(...)` — возвращаемого типа нет.
        let ret = if self.at_punct("{") || self.at_kw("mut") {
            Type::Void
        } else {
            self.parse_type()?
        };

        // mut-контракт
        let mut mut_paths = Vec::new();
        if self.at_kw("mut") {
            self.bump();
            self.expect_punct("(")?;
            loop {
                let p = self.parse_path()?;
                mut_paths.push(MutPath { param: p.root, segs: p.segs });
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
        }

        let body = self.parse_block()?;
        Ok(FuncDecl { name, params, ret, mut_paths, body, owner, line })
    }

    fn parse_param(&mut self) -> PResult<Param> {
        // `self` — первый параметр метода. Тип выводится из `impl`-блока,
        // но синтаксис требует его явно: `self &T` или `self T`.
        let name = self.ident()?;

        // `x in path` — параметр-внутри.
        if self.at_kw("in") {
            self.bump();
            let path = self.parse_path()?;
            let mutable = self.eat_kw("mut");
            let ty = match &path.segs.last() {
                Some(Seg::Elems) => Type::Named("<element>".to_string()),
                _ => Type::Named("<element>".to_string()),
            };
            return Ok(Param { name, ty, kind: ParamKind::RefIn(path), mutable });
        }

        // `x &T` — ссылка; `x T` — владение.
        let is_ref = self.eat_punct("&");
        let ty = self.parse_type()?;
        let mutable = self.eat_kw("mut");
        let kind = if is_ref { ParamKind::Ref } else { ParamKind::Owned };
        Ok(Param { name, ty, kind, mutable })
    }

    fn parse_path(&mut self) -> PResult<Path> {
        let root = self.ident()?;
        let mut segs = Vec::new();
        loop {
            if self.at_punct(".") {
                self.bump();
                let f = self.ident()?;
                segs.push(Seg::Field(f));
            } else if self.at_punct("[") && matches!(self.peek_at(1), Tok::Punct(s) if s == "]") {
                self.bump();
                self.bump();
                segs.push(Seg::Elems);
            } else {
                break;
            }
        }
        Ok(Path { root, segs })
    }

    /// Похоже ли текущее место на объявление переменной без `let`?
    ///
    /// Признак: `имя` + тип (`int`, `bool`, `string`, `void`, `&T`, `[T; N]`
    /// или имя структуры) + `=`. Именно сочетание «имя, затем тип» отличает
    /// забытое объявление от присваивания `x = 5;` и от вызова `f(x);`.
    ///
    /// Тип здесь распознаётся грубо — по форме токенов, а не разбором:
    /// цель в том, чтобы дать подсказку, а не заменить парсер.
    fn looks_like_decl_without_let(&self) -> bool {
        // После имени должен идти тип.
        match self.peek_at(1) {
            // `x &T = ...`
            Tok::Punct(p) if p == "&" => {}
            // `x [T; N] = ...` — но не `arr[0] = ...`: у типа внутри
            // скобок есть `;`, у индекса — нет.
            Tok::Punct(p) if p == "[" => {
                let mut j = 2usize;
                let mut has_semi = false;
                while j < 10 {
                    match self.peek_at(j) {
                        Tok::Punct(q) if q == ";" => {
                            has_semi = true;
                            break;
                        }
                        Tok::Punct(q) if q == "]" => break,
                        Tok::Eof => return false,
                        _ => {}
                    }
                    j += 1;
                }
                if !has_semi {
                    return false;
                }
            }
            // `x int = ...`, `x User = ...`
            Tok::Ident(t) => {
                // Имя типа не должно быть тем, что превращает строку
                // в выражение: `x in ...`, `x mut` и подобное.
                if matches!(t.as_str(), "in" | "mut" | "else") {
                    return false;
                }
            }
            _ => return false,
        }
        // И через один-два токена — `=`. Для `[T; N]` тип длиннее,
        // поэтому смотрим вперёд до `=` в пределах строки. Внутри
        // квадратных скобок `;` — часть типа, а не конец оператора,
        // поэтому считаем глубину. Если тип начинается с `[`, глубина
        // уже равна единице — иначе `;` внутри `[T; N]` обрывает поиск.
        let mut i = 2usize;
        let mut depth = if matches!(self.peek_at(1), Tok::Punct(p) if p == "[") {
            1i32
        } else {
            0i32
        };
        while i < 12 {
            match self.peek_at(i) {
                Tok::Punct(p) if p == "=" && depth == 0 => return true,
                Tok::Punct(p) if p == "[" => depth += 1,
                Tok::Punct(p) if p == "]" => depth -= 1,
                Tok::Punct(p) if (p == ";" || p == ")" || p == "{") && depth <= 0 => {
                    return false
                }
                Tok::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_type(&mut self) -> PResult<Type> {
        if self.eat_punct("&") {
            let inner = self.parse_type()?;
            return Ok(Type::Ref(Box::new(inner)));
        }
        if self.eat_punct("[") {
            let elem = self.parse_type()?;
            self.expect_punct(";")?;
            let n = match self.bump() {
                Tok::Int(n) => n as usize,
                other => {
                    return Err(format!(
                        "строка {}: ожидался размер массива, получено {}",
                        self.line(),
                        self.describe(&other)
                    ))
                }
            };
            self.expect_punct("]")?;
            return Ok(Type::Array(Box::new(elem), n));
        }
        let name = self.ident()?;
        match name.as_str() {
            "int" => Ok(Type::Int),
            "bool" => Ok(Type::Bool),
            "string" => Ok(Type::Str),
            "void" => Ok(Type::Void),
            "Vec" => Err(format!(
                "строка {}: обобщённые типы не поддерживаются в MVP, используйте массивы [T; N]",
                self.line()
            )),
            _ => Ok(Type::Named(name)),
        }
    }

    fn parse_block(&mut self) -> PResult<Block> {
        self.expect_punct("{")?;
        let mut stmts = Vec::new();
        while !self.at_punct("}") {
            stmts.push(self.parse_stmt()?);
        }
        self.expect_punct("}")?;
        Ok(Block { stmts })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        let line = self.line();

        // Похоже на объявление без `let`: `x int = 0;` вместо
        // `let x int = 0;`. Без этой подсказки человек получает
        // «ожидалось ';', получено 'int'» — сообщение о том, чего ждёт
        // парсер, а не о том, что забыл человек.
        if let Tok::Ident(name) = self.peek().clone() {
            // Проверка идёт только для имени, которое не является
            // ключевым словом: иначе `let x = 5;` даёт подсказку
            // «пропущено `let`» про сам `let`.
            let is_keyword = matches!(
                name.as_str(),
                "let" | "fn" | "struct" | "impl" | "if" | "else" | "while"
                    | "for" | "do" | "return" | "break" | "continue"
                    | "in" | "mut" | "true" | "false"
            );
            if !is_keyword && self.looks_like_decl_without_let() {
                return Err(format!(
                    "строка {}: похоже, пропущено `let`: напишите `let {} ...` \
                     — объявление переменной начинается с `let`",
                    line, name
                ));
            }
        }

        if self.at_kw("let") {
            self.bump();
            let mutable = self.eat_kw("mut");
            let name = self.ident()?;
            // Необязательный тип после имени.
            let ty = if self.at_punct("=") {
                None
            } else {
                Some(self.parse_type()?)
            };
            self.expect_punct("=")?;
            let init = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::Let { name, mutable, ty, init });
        }

        if self.at_kw("return") {
            self.bump();
            if self.eat_punct(";") {
                return Ok(Stmt::Return(None));
            }
            let e = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::Return(Some(e)));
        }

        if self.at_kw("if") {
            // Сначала пробуем форму выражения: `if cond { a } else { b }`.
            // Если ветви — не одиночные выражения, откатываемся к форме
            // оператора (`if cond { stmts } else { stmts }`).
            let save = (self.pos, self.next_id);
            match self.parse_if_expr() {
                Ok(e) => return Ok(Stmt::Expr(e)),
                Err(_) => {
                    self.pos = save.0;
                    self.next_id = save.1;
                }
            }
            self.bump();
            let cond = self.parse_expr()?;
            let then_block = self.parse_block()?;
            let else_block = if self.eat_kw("else") {
                if self.at_kw("if") {
                    let nested = self.parse_stmt()?;
                    Some(Block { stmts: vec![nested] })
                } else {
                    Some(self.parse_block()?)
                }
            } else {
                None
            };
            return Ok(Stmt::If { cond, then_block, else_block });
        }

        if self.at_kw("do") {
            self.bump();
            let body = self.parse_block()?;
            self.expect_kw("while")?;
            let cond = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::DoWhile { body, cond });
        }

        if self.at_kw("while") {
            self.bump();
            let cond = self.parse_expr()?;
            let body = self.parse_block()?;
            return Ok(Stmt::While { cond, body });
        }

        if self.at_kw("break") {
            let line = self.line();
            self.bump();
            self.expect_punct(";")?;
            return Ok(Stmt::Break { line });
        }

        if self.at_kw("continue") {
            let line = self.line();
            self.bump();
            self.expect_punct(";")?;
            return Ok(Stmt::Continue { line });
        }

        if self.at_kw("for") {
            let line = self.line();
            self.bump();
            let var = self.ident()?;
            self.expect_kw("in")?;
            // Диапазон: `for i in 0..10`.
            let first = self.parse_expr()?;
            if self.at_punct("..") {
                self.bump();
                let end = self.parse_expr()?;
                let body = self.parse_block()?;
                return Ok(Stmt::ForRange { line, var, start: first, end, body });
            }
            // Массив: `for x in arr`.
            let body = self.parse_block()?;
            return Ok(Stmt::ForEach { line, var, iter: first, body });
        }

        // Выражение или присваивание.
        let lhs = self.parse_expr()?;
        if self.at_punct("=") {
            self.bump();
            let rhs = self.parse_expr()?;
            self.expect_punct(";")?;
            return Ok(Stmt::Assign { lhs, rhs });
        }
        // Составное присваивание: `x += e` → `x = x + e`.
        // Десугарь здесь, в парсере: проверка и кодогенерация не меняются.
        for (punct, op) in [
            ("+=", BinOp::Add),
            ("-=", BinOp::Sub),
            ("*=", BinOp::Mul),
            ("/=", BinOp::Div),
            ("%=", BinOp::Rem),
        ] {
            if self.at_punct(punct) {
                self.bump();
                let rhs = self.parse_expr()?;
                self.expect_punct(";")?;
                let line = lhs.line;
                // Копия левой части — с новым id, чтобы не совпадала с
                // исходным выражением в дереве.
                let lhs_copy = Expr { id: self.id(), line, kind: lhs.kind.clone() };
                let bin = Expr {
                    id: self.id(),
                    line,
                    kind: ExprKind::Binary {
                        op,
                        l: Box::new(lhs_copy),
                        r: Box::new(rhs),
                    },
                };
                return Ok(Stmt::Assign { lhs, rhs: bin });
            }
        }
        self.expect_punct(";")?;
        let _ = line;
        Ok(Stmt::Expr(lhs))
    }

    fn parse_expr(&mut self) -> PResult<Expr> {
        // `if` как выражение — наименьший приоритет: оно не входит в
        // состав бинарных операций (`1 + if ...` не допускается).
        if self.at_kw("if") {
            return self.parse_if_expr();
        }
        self.parse_or()
    }

    /// `if cond { expr } else { expr }` — значение выбранной ветви.
    /// Ветви — одиночные выражения: блоки с операторами разбираются формой
    /// оператора `if`.
    fn parse_if_expr(&mut self) -> PResult<Expr> {
        let line = self.line();
        self.expect_kw("if")?;
        let cond = self.parse_expr()?;
        self.expect_punct("{")?;
        let then_expr = self.parse_expr()?;
        self.expect_punct("}")?;
        self.expect_kw("else")?;
        self.expect_punct("{")?;
        let else_expr = self.parse_expr()?;
        self.expect_punct("}")?;
        Ok(self.mk(
            line,
            ExprKind::If {
                cond: Box::new(cond),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            },
        ))
    }

    fn parse_or(&mut self) -> PResult<Expr> {
        let mut left = self.parse_and()?;
        while self.at_punct("||") {
            let line = self.line();
            self.bump();
            let right = self.parse_and()?;
            let id = self.id();
            left = Expr { id, line, kind: ExprKind::Binary { op: BinOp::Or, l: Box::new(left), r: Box::new(right) } };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> PResult<Expr> {
        let mut left = self.parse_cmp()?;
        while self.at_punct("&&") {
            let line = self.line();
            self.bump();
            let right = self.parse_cmp()?;
            let id = self.id();
            left = Expr { id, line, kind: ExprKind::Binary { op: BinOp::And, l: Box::new(left), r: Box::new(right) } };
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> PResult<Expr> {
        let mut left = self.parse_add()?;
        loop {
            let op = if self.at_punct("==") {
                BinOp::Eq
            } else if self.at_punct("!=") {
                BinOp::Ne
            } else if self.at_punct("<") {
                BinOp::Lt
            } else if self.at_punct(">") {
                BinOp::Gt
            } else if self.at_punct("<=") {
                BinOp::Le
            } else if self.at_punct(">=") {
                BinOp::Ge
            } else {
                break;
            };
            let line = self.line();
            self.bump();
            let right = self.parse_add()?;
            let id = self.id();
            left = Expr { id, line, kind: ExprKind::Binary { op, l: Box::new(left), r: Box::new(right) } };
        }
        Ok(left)
    }

    fn parse_add(&mut self) -> PResult<Expr> {
        let mut left = self.parse_mul()?;
        loop {
            let op = if self.at_punct("+") {
                BinOp::Add
            } else if self.at_punct("-") {
                BinOp::Sub
            } else {
                break;
            };
            let line = self.line();
            self.bump();
            let right = self.parse_mul()?;
            let id = self.id();
            left = Expr { id, line, kind: ExprKind::Binary { op, l: Box::new(left), r: Box::new(right) } };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> PResult<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let op = if self.at_punct("*") {
                BinOp::Mul
            } else if self.at_punct("/") {
                BinOp::Div
            } else if self.at_punct("%") {
                BinOp::Rem
            } else {
                break;
            };
            let line = self.line();
            self.bump();
            let right = self.parse_unary()?;
            let id = self.id();
            left = Expr { id, line, kind: ExprKind::Binary { op, l: Box::new(left), r: Box::new(right) } };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let line = self.line();
        if self.at_punct("-") {
            self.bump();
            let e = self.parse_unary()?;
            let id = self.id();
            return Ok(Expr { id, line, kind: ExprKind::Unary { op: UnOp::Neg, e: Box::new(e) } });
        }
        if self.at_punct("!") {
            self.bump();
            let e = self.parse_unary()?;
            let id = self.id();
            return Ok(Expr { id, line, kind: ExprKind::Unary { op: UnOp::Not, e: Box::new(e) } });
        }
        if self.at_punct("&") {
            self.bump();
            let e = self.parse_unary()?;
            let id = self.id();
            return Ok(Expr { id, line, kind: ExprKind::Ref(Box::new(e)) });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            let line = self.line();
            if self.at_punct(".") {
                self.bump();
                let name = self.ident()?;
                // `x.method(args)` — вызов метода; `x.field` — доступ к полю.
                if self.at_punct("(") {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.at_punct(")") {
                        args.push(self.parse_expr()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                    self.expect_punct(")")?;
                    let id = self.id();
                    e = Expr {
                        id,
                        line,
                        kind: ExprKind::MethodCall { obj: Box::new(e), name, args },
                    };
                } else {
                    let id = self.id();
                    e = Expr { id, line, kind: ExprKind::Field { obj: Box::new(e), name } };
                }
            } else if self.at_punct("[") {
                self.bump();
                let idx = self.parse_expr()?;
                self.expect_punct("]")?;
                let id = self.id();
                e = Expr { id, line, kind: ExprKind::Index { obj: Box::new(e), idx: Box::new(idx) } };
            } else if self.at_punct("(") {
                self.bump();
                let mut args = Vec::new();
                while !self.at_punct(")") {
                    args.push(self.parse_expr()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
                let id = self.id();
                e = Expr { id, line, kind: ExprKind::Call { callee: Box::new(e), args } };
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(self.mk(line, ExprKind::Int(n)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(self.mk(line, ExprKind::Str(s)))
            }
            Tok::Ident(s) => {
                self.bump();
                // `Type::name` — ассоциированная функция. Склеиваем в
                // одно имя: `Point::new`.
                if self.at_punct("::") {
                    self.bump();
                    let member = self.ident()?;
                    let full = format!("{}::{}", s, member);
                    return Ok(self.mk(line, ExprKind::Ident(full)));
                }
                match s.as_str() {
                    "true" => Ok(self.mk(line, ExprKind::Bool(true))),
                    "false" => Ok(self.mk(line, ExprKind::Bool(false))),
                    "if" => Err(format!(
                        "строка {}: `if` как выражение используется целиком: \
                         `1 + if ...` не допускается",
                        line
                    )),
                    _ => Ok(self.mk(line, ExprKind::Ident(s))),
                }
            }
            Tok::Punct(p) if p == "(" => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            Tok::Punct(p) if p == "[" => {
                // Массив-литерал: [a, b, c]
                self.bump();
                let mut items = Vec::new();
                while !self.at_punct("]") {
                    items.push(self.parse_expr()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct("]")?;
                let id = self.id();
                Ok(Expr { id, line, kind: ExprKind::Array(items) })
            }
            other => Err(format!(
                "строка {}: ожидалось выражение, получено {}",
                line,
                self.describe(&other)
            )),
        }
    }
}
