//! Keel — компилятор (MVP).
//!
//! Пайплайн: лексер → парсер → проверщик заимствований → генерация C.

pub mod ast;
pub mod checker;
pub mod codegen;
pub mod lexer;
pub mod parser;

use ast::Program;
use checker::{Checker, Diagnostic};

pub struct CompileResult {
    pub program: Program,
    pub c_code: String,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn compile(src: &str) -> Result<CompileResult, String> {
    let mut p = parser::Parser::new(src)?;
    let program = p.parse_program()?;

    let mut checker = Checker::new();
    let diagnostics = checker.check(&program);

    if !diagnostics.is_empty() {
        return Ok(CompileResult { program, c_code: String::new(), diagnostics });
    }

    let c_code = codegen::Codegen::new(&program, &checker).generate();
    Ok(CompileResult { program, c_code, diagnostics })
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
