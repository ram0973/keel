//! CLI компилятора Keel.

use keelc::{compile, format_diagnostics};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("использование:");
        eprintln!("  keelc check <file.keel>          — только проверка");
        eprintln!("  keelc build <file.keel> <out.c>  — сгенерировать C");
        eprintln!("  keelc run   <file.keel>          — скомпилировать и запустить");
        std::process::exit(2);
    }

    let cmd = args[1].clone();
    let path = args[2].clone();

    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("не удалось прочитать {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let result = match compile(&src) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ошибка разбора: {}", e);
            std::process::exit(1);
        }
    };

    if !result.diagnostics.is_empty() {
        eprint!("{}", format_diagnostics(&result.diagnostics));
        eprintln!("ошибок: {}", result.diagnostics.len());
        std::process::exit(1);
    }

    match cmd.as_str() {
        "check" => {
            println!("проверка пройдена: {}", path);
        }
        "build" => {
            let out = args.get(3).cloned().unwrap_or_else(|| "out.c".to_string());
            if let Err(e) = std::fs::write(&out, &result.c_code) {
                eprintln!("не удалось записать {}: {}", out, e);
                std::process::exit(1);
            }
            println!("сгенерирован {}", out);
        }
        "run" => {
            // Уникальные имена: иначе параллельные запуски (или запуск от
            // другого пользователя) столкнутся за один и тот же файл.
            let pid = std::process::id();
            let tmp_c = std::env::temp_dir().join(format!("keel_out_{}.c", pid));
            let tmp_bin = std::env::temp_dir().join(format!("keel_out_{}.bin", pid));
            let _ = std::fs::write(&tmp_c, &result.c_code);

            // Ищем компилятор C.
            let cc = if cfg!(target_os = "windows") { "clang" } else { "cc" };
            let st = Command::new(cc)
                .arg("-w")
                // Оптимизация: без неё сгенерированный C компилируется
                // как -O0, и код Keel заметно медленее того, что может
                // дать gcc. Уровень можно переопределить переменной
                // KEEL_OPT (например, KEEL_OPT=-O0 для отладки).
                .arg(std::env::var("KEEL_OPT").unwrap_or_else(|_| "-O2".to_string()))
                .arg("-o")
                .arg(&tmp_bin)
                .arg(&tmp_c)
                .status();
            match st {
                Ok(s) if s.success() => {}
                Ok(_) => {
                    eprintln!("ошибка компиляции C");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("не удалось запустить `{}`: {}", cc, e);
                    std::process::exit(1);
                }
            }
            let st = Command::new(&tmp_bin).status();
            match st {
                Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                Err(e) => {
                    eprintln!("не удалось запустить бинарник: {}", e);
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("неизвестная команда: {}", other);
            std::process::exit(2);
        }
    }
}
