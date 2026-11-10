// Тесты: проверка идей group borrowing, а не компилятора вообще.
//
// Каждый тест — пара: программа, которая ДОЛЖНА компилироваться, и
// программа, которая ДОЛЖНА падать с конкретной ошибкой.

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

// ============================================================
// 1. Базовое владение
// ============================================================

#[test]
fn простое_присваивание_работает() {
    ok(r#"
fn main() int {
    let mut x = 1;
    x = 2;
    return x;
}
"#);
}

#[test]
fn изменяемую_переменную_нельзя_переприсваивать_без_mut() {
    fails(
        r#"
fn main() int {
    let x = 1;
    x = 2;
    return x;
}
"#,
        "не объявлен как `mut`",
    );
}

#[test]
fn перемещённое_значение_нельзя_использовать() {
    fails(
        r#"
struct Point { x int; y int; }
fn take(p Point) int { return p.x; }
fn main() int {
    let p = Point(1, 2);
    let q = take(p);
    return p.x;
}
"#,
        "уже перемещён",
    );
}

#[test]
fn целые_числа_копируются_а_не_перемещаются() {
    ok(r#"
fn take(n int) int { return n; }
fn main() int {
    let x = 5;
    let y = take(x);
    return x + y;
}
"#);
}

// ============================================================
// 2. Ключевая идея: ссылки помнят путь
// ============================================================

#[test]
fn ссылка_на_поле_структуры_живёт_пока_жива_структура() {
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let e = Entity(42);
    let r = &e.hp;
    print(r);
    return 0;
}
"#);
}

#[test]
fn ссылка_инвалидируется_при_замене_поля() {
    // Это и есть use-after-free, пойманный на этапе компиляции.
    fails(
        r#"
struct Entity { hp int; }
fn main() int {
    let mut e = Entity(42);
    let r = &e.hp;
    e.hp = 7;
    print(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn ссылка_на_весь_объект_не_инвалидируется_при_изменении_поля() {
    // Путь «внутрь», а не «на»: замена поля не убивает ссылку на объект.
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut e = Entity(42);
    let r = &e;
    e.hp = 7;
    print(r.hp);
    return 0;
}
"#);
}

#[test]
fn ссылка_на_поле_другого_объекта_не_затрагивается() {
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut a = Entity(1);
    let mut b = Entity(2);
    let r = &b.hp;
    a.hp = 100;
    print(r);
    return 0;
}
"#);
}

// ============================================================
// 3. То, что Rust запрещает, а Keel разрешает
// ============================================================

#[test]
fn две_изменяемые_ссылки_на_элементы_одной_коллекции_разрешены() {
    // Rust: E0499 — нельзя заимствовать entities дважды изменяемо.
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut arr = [Entity(1), Entity(2)];
    let a = &arr[0];
    let b = &arr[1];
    a.hp = 10;
    b.hp = 20;
    print(a.hp);
    return 0;
}
"#);
}

#[test]
fn несколько_изменяемых_ссылок_на_один_объект_разрешены() {
    // Rust: нельзя иметь две &mut на одно и то же.
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut e = Entity(42);
    let a = &e;
    let b = &e;
    a.hp = 10;
    b.hp = 20;
    print(e.hp);
    return 0;
}
"#);
}

#[test]
fn ссылка_на_элемент_и_на_контейнер_одновременно_разрешены() {
    // То, ради чего в Rust нужны lifetime-аннотации или перезапрос ссылок.
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut arr = [Entity(1), Entity(2)];
    let elem = &arr[0];
    let whole = &arr;
    elem.hp = 10;
    print(whole[1].hp);
    return 0;
}
"#);
}

// ============================================================
// 4. mut-контракт функции
// ============================================================

#[test]
fn функция_должна_объявить_что_она_меняет() {
    fails(
        r#"
struct Entity { hp int; }
fn damage(e &Entity) {
    e.hp = e.hp - 1;
}
fn main() int {
    let mut e = Entity(42);
    damage(&e);
    return 0;
}
"#,
        "mut-контракте",
    );
}

#[test]
fn корректный_mut_контракт_принимается() {
    ok(r#"
struct Entity { hp int; }
fn damage(e &Entity) mut(e) {
    e.hp = e.hp - 1;
}
fn main() int {
    let mut e = Entity(42);
    damage(&e);
    return 0;
}
"#);
}

#[test]
fn вызов_функции_инвалидирует_ссылки_внутрь_изменённого() {
    // Если damage() объявлена как меняющая содержимое e, то ссылка на
    // e.hp у вызывающего становится недействительной.
    fails(
        r#"
struct Entity { hp int; }
fn damage(e &Entity) mut(e) {
    e.hp = e.hp - 1;
}
fn main() int {
    let mut e = Entity(42);
    let r = &e.hp;
    damage(&e);
    print(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// ============================================================
// 5. Циклы
// ============================================================

#[test]
fn ссылка_переживает_итерацию_если_не_инвалидируется() {
    ok(r#"
struct Entity { hp int; }
fn main() int {
    let mut total = 0;
    let mut i = 0;
    while i < 3 {
        total = total + i;
        i = i + 1;
    }
    print(total);
    return 0;
}
"#);
}

#[test]
fn использование_ссылки_после_инвалидации_в_цикле_ловится() {
    fails(
        r#"
struct Entity { hp int; }
fn main() int {
    let mut e = Entity(42);
    let mut i = 0;
    while i < 3 {
        let r = &e.hp;
        e.hp = i;
        print(r);
        i = i + 1;
    }
    return 0;
}
"#,
        "инвалидирована",
    );
}

// ============================================================
// 6. Типы
// ============================================================

#[test]
fn несовпадение_типов_ловится() {
    fails(
        r#"
fn main() int {
    let x int = "строка";
    return 0;
}
"#,
        "несовпадение типов",
    );
}

#[test]
fn неизвестное_поле_ловится() {
    fails(
        r#"
struct Entity { hp int; }
fn main() int {
    let e = Entity(42);
    print(e.armor);
    return 0;
}
"#,
        "нет поля",
    );
}

#[test]
fn неизвестная_функция_ловится() {
    fails(
        r#"
fn main() int {
    return nonexistent();
}
"#,
        "неизвестная функция",
    );
}

// ============================================================
// 7. Ссылка на параметр-внутри
// ============================================================

#[test]
fn параметр_внутри_даёт_доступ_к_элементу() {
    // `e in entities[]` — ссылка на элемент массива, переданного
    // параметром-ссылкой `entities`.
    ok(r#"
struct Entity { hp int; }
fn damage(entities &[Entity; 2], e in entities[] mut) {
    e.hp = e.hp - 1;
}
fn main() int {
    let mut arr = [Entity(1), Entity(2)];
    damage(&arr, &arr[0]);
    print(arr[0].hp);
    return 0;
}
"#);
}
