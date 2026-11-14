// Проверка замечаний ревью checker.rs. Каждый тест — гипотеза из ревью.
// Пока тест падает — дыра реальна; после исправления он должен проходить.

use keelc::{compile, format_diagnostics};

fn ok(src: &str) {
    let r = match compile(src) {
        Ok(r) => r,
        Err(e) => panic!("ошибка разбора: {}", e),
    };
    assert!(
        r.diagnostics.is_empty(),
        "ожидался успех, получены ошибки:\n{}",
        format_diagnostics(&r.diagnostics)
    );
}

fn fails(src: &str, expected: &str) {
    let r = match compile(src) {
        Ok(r) => r,
        Err(e) => panic!("ошибка разбора: {}", e),
    };
    let text = format_diagnostics(&r.diagnostics);
    assert!(
        !r.diagnostics.is_empty(),
        "ожидалась ошибка, содержащая `{}`, но программа скомпилировалась",
        expected
    );
    assert!(
        text.contains(expected),
        "ожидалась ошибка с `{}`, получено:\n{}",
        expected,
        text
    );
}

// 1a. Результат вызова-ссылки не отслеживается.
#[test]
fn call_result_ref_not_tracked() {
    fails(
        r#"
struct W { v int; }
fn first(w &W) &int { return &w.v; }
fn main() int {
    let mut w = W(1);
    let r = first(&w);
    w.v = 2;
    println(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 1b. if-выражение-ссылка не отслеживается.
#[test]
fn if_expr_ref_not_tracked() {
    fails(
        r#"
fn main() int {
    let mut a = 1;
    let mut b = 2;
    let c = true;
    let r = if c { &a } else { &b };
    a = 3;
    println(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 2. Анализ выхода не видит вызовов.
#[test]
fn escape_through_call() {
    fails(
        r#"
fn id(r &int) &int { return r; }
fn bad() &int {
    let i = 1;
    return id(&i);
}
fn main() int { return 0; }
"#,
        "локальную",
    );
}

// 3. Сохранение ссылки через mut-параметр.
#[test]
fn store_ref_through_mut_param() {
    fails(
        r#"
struct H { r &int; }
fn store(h &H, r &int) mut(h) { h.r = r; }
fn bad(h &H) mut(h) {
    let i = 1;
    store(h, &i);
}
fn main() int { return 0; }
"#,
        "локальную",
    );
}

// 4. Присваивание в локальную структуру не обновляет escapes.
#[test]
fn assign_into_local_struct_escape() {
    fails(
        r#"
struct H { r &int; }
fn bad(p &int) H {
    let i = 1;
    let mut h = H(p);
    h.r = &i;
    return h;
}
fn main() int { return 0; }
"#,
        "локальную",
    );
}

// 5. Move не инвалидирует ссылки.
#[test]
fn move_invalidates_refs() {
    fails(
        r#"
struct S { x int; }
fn consume(s S) { }
fn main() int {
    let a = S(1);
    let r = &a;
    consume(a);
    println(r.x);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 8. return r.x, где r — ссылка на локальную, x — int: ложное срабатывание.
#[test]
fn return_field_of_ref_to_local_is_ok() {
    ok(
        r#"
struct S { x int; }
fn bad() int {
    let s = S(1);
    let r = &s;
    return r.x;
}
fn main() int { return 0; }
"#,
    );
}

// 9. Метод: получатель проверяется дважды.
#[test]
fn method_receiver_not_double_checked() {
    ok(
        r#"
struct S { x int; }
impl S {
    fn get(self &S) int { return self.x; }
}
fn consume(s S) S { return s; }
fn main() int {
    let s = S(1);
    let v = consume(s).get();
    println(v);
    return 0;
}
"#,
    );
}

// 11. Типы элементов массива не сверяются.
#[test]
fn array_element_types_checked() {
    fails(
        r#"
fn main() int {
    let xs = [1, true];
    return 0;
}
"#,
        "несовпадение",
    );
}

// 6a. Move в then не даёт ложное «уже перемещён» в else.
#[test]
fn if_else_branches_independent() {
    ok(
        r#"
struct S { x int; }
fn consume(s S) int { return s.x; }
fn main() int {
    let a = S(1);
    let c = true;
    let v = if c { consume(a) } else { consume(a) };
    println(v);
    return 0;
}
"#,
    );
}

// 6b. do-while: условие выполняется после тела.
#[test]
fn do_while_condition_after_body() {
    fails(
        r#"
fn main() int {
    let mut a = 1;
    let r = &a;
    do {
        a = 2;
    } while (r > 0);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 6c. while: условие перепроверяется после тела.
#[test]
fn while_condition_rechecked() {
    fails(
        r#"
fn main() int {
    let mut a = 1;
    let r = &a;
    while (r > 0) {
        a = 2;
    }
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 10. Повторная инициализация перемещённой let mut.
#[test]
fn reassign_moved_let_mut() {
    ok(
        r#"
struct S { x int; }
fn consume(s S) int { return s.x; }
fn main() int {
    let mut a = S(1);
    let v = consume(a);
    a = S(2);
    println(v);
    return 0;
}
"#,
    );
}

// 11. Условие if должно быть bool.
#[test]
fn if_condition_must_be_bool() {
    fails(
        r#"
fn main() int {
    if 1 { return 0; }
    return 1;
}
"#,
        "bool",
    );
}

// 12. Утечка из then не теряется при слиянии ветвей.
#[test]
fn if_then_escape_merged() {
    fails(
        r#"
struct H { r &int; }
fn bad(p &int, c bool) H {
    let mut h = H(p);
    let i = 1;
    if c { h.r = &i; }
    return h;
}
fn main() int { return 0; }
"#,
        "локальную",
    );
}

// 13. x = f(x) после consume(x) — использование перемещённого.
#[test]
fn reassign_uses_moved_rhs() {
    fails(
        r#"
struct S { x int; }
fn consume(s S) int { return s.x; }
fn f(x S) S { return x; }
fn main() int {
    let mut x = S(1);
    consume(x);
    x = f(x);
    return 0;
}
"#,
        "перемещён",
    );
}

// 14. Цель ссылки — все ref-аргументы, а не первый с путём.
#[test]
fn call_ref_targets_all_ref_args() {
    fails(
        r#"
fn pick(count int, a &int, b &int) &int { return a; }
fn main() int {
    let mut x = 1;
    let mut y = 2;
    let r = pick(0, &x, &y);
    y = 5;
    println(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 15. Метод-ссылка отслеживается.
#[test]
fn method_ref_target_tracked() {
    fails(
        r#"
struct W { items [int; 2]; }
impl W {
    fn first(self &W) &int { return &self.items[0]; }
}
fn main() int {
    let mut w = W([1, 2]);
    let r = w.first();
    w.items[0] = 3;
    println(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 16. Срез — заимствование массива: мутация гасит.
#[test]
fn slice_tracks_array() {
    fails(
        r#"
fn main() int {
    let mut arr [int; 3] = [1, 2, 3];
    let s [int] = arr;
    arr[0] = 9;
    println(s[0]);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// 17. store(&h, &i) кладёт ссылку в локальный контейнер.
#[test]
fn store_into_local_container() {
    fails(
        r#"
struct H { r &int; }
fn store(h &H, r &int) mut(h) { h.r = r; }
fn bad(p &int) H {
    let mut h = H(p);
    let i = 1;
    store(&h, &i);
    return h;
}
fn main() int { return 0; }
"#,
        "локальную",
    );
}

// 18. Обе ветви перемещают — после if переменная перемещена, но
// использования после нет: ошибки быть не должно.
#[test]
fn if_both_branches_move_ok() {
    ok(
        r#"
struct S { x int; }
fn consume(s S) int { return s.x; }
fn main() int {
    let a = S(1);
    let c = true;
    if c { consume(a); } else { consume(a); }
    return 0;
}
"#,
    );
}

// 19. Ветвь с return не вносит своё состояние.
#[test]
fn if_diverging_branch_ignored() {
    ok(
        r#"
struct S { x int; }
fn consume(s S) int { return s.x; }
fn main() int {
    let a = S(1);
    let bad = true;
    if bad { consume(a); return 0; }
    println(a.x);
    return 0;
}
"#,
    );
}
