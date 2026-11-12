//! Keel — компилятор (MVP).
//!
//! Пайплайн: загрузка модулей → лексер → парсер → проверщик заимствований
//! → генерация C.

pub mod ast;
pub mod checker;
pub mod codegen;
pub mod lexer;
pub mod parser;

use ast::{Item, Program};
use checker::{Checker, Diagnostic};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct CompileResult {
    pub program: Program,
    pub c_code: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Компилирует текст без загрузки модулей.
///
/// `import` при этом остаётся в дереве, но ничего не подключает —
/// так работают тесты, которым нужен один файл.
pub fn compile(src: &str) -> Result<CompileResult, String> {
    let mut p = parser::Parser::new(src)?;
    let program = p.parse_program()?;
    finish(program)
}

/// Компилирует файл вместе со всеми `import`.
///
/// Пути в `import` считаются относительно каталога импортирующего файла.
/// Один файл подключается один раз, даже если импортирован из нескольких
/// мест. Циклы обнаруживаются и сообщаются как ошибка.
pub fn compile_file(path: &Path) -> Result<CompileResult, String> {
    let mut loader = Loader::new();
    let program = loader.load(path)?;
    finish(program)
}

fn finish(program: Program) -> Result<CompileResult, String> {
    let mut checker = Checker::new();
    let diagnostics = checker.check(&program);

    if !diagnostics.is_empty() {
        return Ok(CompileResult { program, c_code: String::new(), diagnostics });
    }

    let c_code = codegen::Codegen::new(&program, &checker).generate();
    Ok(CompileResult { program, c_code, diagnostics })
}

/// Загрузчик модулей: читает файлы, следит за циклами и повторами.
struct Loader {
    /// Уже загруженные файлы (канонизированные пути) — чтобы один файл
    /// не подключался дважды.
    loaded: HashSet<PathBuf>,
    /// Стек текущей цепочки импортов — для обнаружения циклов.
    stack: Vec<PathBuf>,
    /// Следующий свободный id выражения. Счётчик общий на всю программу:
    /// id должны быть уникальны между файлами, иначе типы выражений
    /// перепутаются (см. `Parser::with_id_start`).
    next_id: u32,
}

impl Loader {
    fn new() -> Loader {
        Loader { loaded: HashSet::new(), stack: Vec::new(), next_id: 0 }
    }

    /// Загружает файл и все его импорты, возвращая общий список объявлений.
    ///
    /// Порядок: сначала объявления самого файла, затем — подключённых.
    /// Для проверщика порядок не важен (он собирает все объявления до
    /// проверки тел), но для читаемости кода в C он предсказуем.
    fn load(&mut self, path: &Path) -> Result<Program, String> {
        // Канонизируем, чтобы `./user.keel` и `user.keel` считались одним
        // файлом. Если файла нет — сообщаем прямо.
        let canon = std::fs::canonicalize(path).map_err(|e| {
            format!("не удалось открыть `{}`: {}", path.display(), e)
        })?;

        if self.stack.contains(&canon) {
            let mut chain: Vec<String> = self
                .stack
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.display().to_string())
                })
                .collect();
            chain.push(
                canon
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| canon.display().to_string()),
            );
            return Err(format!(
                "цикл импорта: {}",
                chain.join(" → ")
            ));
        }

        // Уже загружен — второй раз не читаем.
        if self.loaded.contains(&canon) {
            return Ok(Program { items: Vec::new() });
        }

        let src = std::fs::read_to_string(&canon)
            .map_err(|e| format!("не удалось прочитать `{}`: {}", canon.display(), e))?;

        let mut p = parser::Parser::with_id_start(&src, self.next_id)?;
        let program = p.parse_program()?;
        // Продолжаем нумерацию со следующего файла.
        self.next_id = p.next_id();

        self.stack.push(canon.clone());
        self.loaded.insert(canon.clone());

        let dir = canon.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let mut items: Vec<Item> = Vec::new();

        for item in program.items {
            match item {
                // Импорт: загружаем подключённый файл и вливаем его
                // объявления. Сам узел `Import` в дереве не остаётся —
                // он выполнен.
                Item::Import(imp) => {
                    let target = dir.join(&imp.path);
                    let sub = self.load(&target)?;
                    items.extend(sub.items);
                }
                other => items.push(other),
            }
        }

        self.stack.pop();
        Ok(Program { items })
    }
}

pub fn format_diagnostics(diags: &[Diagnostic]) -> String {
    let mut out = String::new();
    for d in diags {
        if d.line > 0 {
            out.push_str(&format!("строка {}: ", d.line));
        }
        out.push_str(&d.msg);
        out.push('\n');
    }
    out
}
