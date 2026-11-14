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

/// Ошибка **разбора**: часть проверок (значения констант, размер массива)
/// выполняется прямо в парсере, до проверщика. Такие ошибки приходят как
/// `Err`, а не как диагностика.
fn parse_fails(src: &str, expected: &str) {
    match compile(src) {
        Err(e) => assert!(
            e.contains(expected),
            "ожидалась ошибка разбора с `{}`, получено: {}",
            expected,
            e
        ),
        Ok(r) => panic!(
            "ожидалась ошибка разбора с `{}`, программа скомпилировалась: {:?}",
            expected, r.diagnostics
        ),
    }
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
    println(r);
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
    println(r);
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
    println(r.hp);
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
    println(r);
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
    println(a.hp);
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
    println(e.hp);
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
    println(whole[1].hp);
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
    println(r);
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
    println(total);
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
        println(r);
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
fn ссылки_сравниваются_по_значению() {
    // Авторазыменование: `x < y` где x, y — ссылки на int.
    ok(r#"
fn main() int {
    let mut a = 1;
    let mut b = 2;
    let x = &a;
    let y = &b;
    if x < y {
        println(1);
    }
    return 0;
}
"#);
}

#[test]
fn авторазыменование_в_арифметике() {
    ok(r#"
fn main() int {
    let mut a = 10;
    let r = &a;
    let s = r + 5;
    println(s);
    return 0;
}
"#);
}

#[test]
fn устаревшая_ссылка_на_запись_ловится() {
    // Сценарий из примера phonebook.keel: взяли ссылку на запись,
    // запись заменили — ссылка устарела.
    fails(
        r#"
struct Entry { id int; name string; }
struct Book { entries [Entry; 2]; count int; }
fn edit(book &Book mut, id int, name string) int {
    book.entries[0] = Entry(id, name);
    return 1;
}
fn main() int {
    let mut book = Book([Entry(1, "a"), Entry(2, "b")], 2);
    let first = &book.entries[0];
    edit(&book, 2, "new");
    println(first.name);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn неизвестное_поле_ловится() {
    fails(
        r#"
struct Entity { hp int; }
fn main() int {
    let e = Entity(42);
    println(e.armor);
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
    println(arr[0].hp);
    return 0;
}
"#);
}

// ============================================================
// 8. Пустой массив
// ============================================================

#[test]
fn пустой_массив_с_явным_типом_компилируется() {
    // Тип элемента пустого литерала вывести нельзя — элементов нет.
    // Поэтому тип объявляется явно, а литерал наследует его.
    ok(r#"
fn main() int {
    let empty [int; 0] = [];
    let empty_entries [Entry; 0] = [];
    return 0;
}
struct Entry { id int; }
"#);
}

#[test]
fn пустой_массив_без_типа_ошибка() {
    // Без объявленного типа элемент пустого массива неизвестен.
    fails(
        r#"
fn main() int {
    let empty = [];
    return 0;
}
"#,
        "невозможно вывести тип элемента пустого массива",
    );
}

// ============================================================
// 9. Составное присваивание
// ============================================================

#[test]
fn составное_присваивание_работает() {
    ok(r#"
fn main() int {
    let mut x = 10;
    x += 5;
    x -= 3;
    x *= 2;
    x /= 4;
    x %= 4;
    return x;
}
"#);
}

#[test]
fn составное_присваивание_на_элементе_и_поле() {
    ok(r#"
struct Point { x int; y int; }
fn main() int {
    let mut a [int; 3] = [1, 2, 3];
    a[1] += 10;
    let mut p = Point(1, 2);
    p.x += 100;
    return a[1] + p.x;
}
"#);
}

#[test]
fn составное_присваивание_на_не_изменяемой_ошибка() {
    // `x += 1` — это `x = x + 1`, поэтому требует `mut`, как и обычное
    // присваивание.
    fails(
        r#"
fn main() int {
    let x = 1;
    x += 1;
    return x;
}
"#,
        "не объявлен как `mut`",
    );
}

#[test]
fn мутация_своего_параметра_не_требует_контракта() {
    // Параметр по значению перемещён в функцию: мутация не видна
    // вызывающему, поэтому объявлять её не нужно.
    ok(r#"
fn bump(x int mut) {
    x += 1;
}
fn main() int {
    return 0;
}
"#);
}

// ============================================================
// 10. `if` как выражение
// ============================================================

#[test]
fn if_как_выражение_работает() {
    ok(r#"
fn max_of(a int, b int) int {
    return if a > b { a } else { b };
}
fn main() int {
    let x = 3;
    let y = 7;
    let m = if x > y { x } else { y };
    return m;
}
"#);
}

#[test]
fn if_как_выражение_со_строками() {
    ok(r#"
fn main() int {
    let a = 1;
    let b = 2;
    let s = if a > b { "greater" } else { "less" };
    return 0;
}
"#);
}

#[test]
fn if_как_выражение_с_разными_типами_ветвей_ошибка() {
    // Ветви должны давать одно значение — иначе тип выражения зависит
    // от того, какая ветвь выполнилась.
    fails(
        r#"
fn main() int {
    let a = 1;
    let x = if a > 0 { 1 } else { "нет" };
    return 0;
}
"#,
        "несовпадение типов в ветвях",
    );
}

#[test]
fn if_в_бинарной_операции_не_допускается() {
    // У `if` как выражения наименьший приоритет: он не входит в состав
    // бинарных операций. Это ошибка разбора, а не проверки.
    let r = match compile(
        r#"
fn main() int {
    let n = 1;
    let s = n + if n > 0 { 1 } else { 2 };
    return 0;
}
"#,
    ) {
        Ok(r) => r,
        Err(e) => {
            assert!(
                e.contains("используется целиком"),
                "ожидалась ошибка про `if` как выражение, получено: {}",
                e
            );
            return;
        }
    };
    panic!(
        "ожидалась ошибка разбора, программа скомпилировалась: {:?}",
        r.diagnostics
    );
}

#[test]
fn оператор_if_продолжает_работать() {
    // Форма оператора (ветви — блоки с операторами) не должна сломаться.
    ok(r#"
fn main() int {
    let mut total = 0;
    let a = 5;
    let b = 8;
    if a > b {
        total = 1;
    } else {
        total = 2;
    }
    return total;
}
"#);
}

// ============================================================
// 11. Циклы
// ============================================================

#[test]
fn for_по_диапазону_работает() {
    ok(r#"
fn main() int {
    let mut total = 0;
    for i in 0..10 {
        total += i;
    }
    return total;
}
"#);
}

#[test]
fn for_по_диапазону_с_границами_переменными() {
    ok(r#"
fn main() int {
    let lo = 2;
    let hi = 5;
    let mut n = 0;
    for i in lo..hi {
        n += 1;
    }
    return n;
}
"#);
}

#[test]
fn for_по_массиву_работает() {
    ok(r#"
fn main() int {
    let a [int; 3] = [1, 2, 3];
    let mut total = 0;
    for v in a {
        total += v;
    }
    return total;
}
"#);
}

#[test]
fn break_и_continue_работают() {
    ok(r#"
fn main() int {
    let mut n = 0;
    for i in 0..10 {
        if i == 3 {
            break;
        }
        if i % 2 == 0 {
            continue;
        }
        n += i;
    }
    return n;
}
"#);
}

#[test]
fn break_вне_цикла_ошибка() {
    fails(
        r#"
fn main() int {
    break;
    return 0;
}
"#,
        "`break` используется вне цикла",
    );
}

#[test]
fn continue_вне_цикла_ошибка() {
    fails(
        r#"
fn main() int {
    continue;
    return 0;
}
"#,
        "`continue` используется вне цикла",
    );
}

#[test]
fn for_по_не_массиву_ошибка() {
    fails(
        r#"
fn main() int {
    let x = 5;
    for v in x {
        println(v);
    }
    return 0;
}
"#,
        "работает по массиву",
    );
}

#[test]
fn границы_диапазона_должны_быть_int() {
    fails(
        r#"
fn main() int {
    for i in 0.."нет" {
        println(i);
    }
    return 0;
}
"#,
        "должна быть int",
    );
}

#[test]
fn вложенные_циклы_с_одним_именем_работают() {
    // В C область действия `for` — всё тело, поэтому внутренний цикл
    // с тем же именем перетирает внешний. Компилятор обязан развести
    // имена.
    ok(r#"
fn main() int {
    let mut n = 0;
    for i in 0..3 {
        for i in 0..3 {
            n += 1;
        }
    }
    return n;
}
"#);
}

#[test]
fn do_while_работает() {
    ok(r#"
fn main() int {
    let mut i = 0;
    do {
        i += 1;
    } while i < 3;
    return i;
}
"#);
}

#[test]
fn println_и_print_работают() {
    // `println` печатает с переводом строки, `print` — без него.
    ok(r#"
fn main() int {
    println(42);
    println("строка");
    println(true);
    print("a");
    print("b");
    return 0;
}
"#);
}

#[test]
fn utf8_строки_работают() {
    // Строки — это байты; кириллица и эмодзи проходят как есть.
    ok(r#"
fn main() int {
    let ru = "Привет, мир";
    let emoji = "👋🌍";
    println(ru);
    println(emoji);
    return 0;
}
"#);
}

#[test]
fn две_ссылки_на_один_объект_разрешены() {
    // В Keel один вид ссылки (`&T`), и право писать определяется
    // владельцем пути, а не видом ссылки. Поэтому запрета на совпадение
    // ссылок нет — в отличие от Rust, где две `&mut` на один объект
    // дают E0499.
    //
    // Запрещено другое: ИСПОЛЬЗОВАНИЕ ссылки после того, как объект
    // на её пути заменили. Это проверяется отдельно.
    ok(r#"
struct Item { v int; }
fn main() int {
    let mut arr [Item; 3] = [Item(0), Item(0), Item(0)];
    let a = &arr[0];
    let b = &arr[0];
    a.v = 5;
    b.v = 6;
    return 0;
}
"#);
}

#[test]
fn ссылка_на_заменённый_объект_ловится() {
    // А вот это — ядро безопасности: объект на пути заменили, ссылка
    // больше недействительна. В C это тихая ошибка.
    fails(
        r#"
struct Item { v int; }
fn main() int {
    let mut arr [Item; 3] = [Item(0), Item(0), Item(0)];
    let a = &arr[0];
    arr[0] = Item(9);
    println(a.v);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn затенение_переменной_работает() {
    // Второе `let x` затеняет первое. В C это «redefinition» — нужно
    // развести имена, иначе сгенерированный код не компилируется.
    ok(r#"
struct Counter { n int; }
fn main() int {
    let a = Counter(10);
    let a = Counter(20);
    println(a.n);
    return 0;
}
"#);
}

#[test]
fn затенение_не_ломает_прежнее_значение() {
    // Первое значение используется ДО затенения — оно должно остаться
    // доступным.
    ok(r#"
struct Counter { n int; }
fn take(c Counter) int { return c.n; }
fn main() int {
    let a = Counter(10);
    let first = take(a);
    let a = Counter(20);
    println(first);
    println(a.n);
    return 0;
}
"#);
}

#[test]
fn структура_с_массивом_в_поле_собирается() {
    // В C нельзя присвоить массив в поле структуры (`.f = arr` — ошибка
    // компиляции). Для таких структур генерируется отдельный конструктор
    // `k_new_Имя`, который копирует поле через memcpy.
    ok(r#"
struct User { name string; age int; }
struct Registry { users [User; 3]; count int; }
fn main() int {
    let users = [User("John", 26), User("Mary", 18), User("James", 18)];
    let reg = Registry(users, 3);
    println(reg.users[0].name);
    println(reg.count);
    return 0;
}
"#);
}

#[test]
fn ассоциированная_функция_вызывается_через_двойное_двоеточие() {
    ok(r#"
struct Point { x int; y int; }
impl Point {
    fn new(x int, y int) Point {
        return Point(x, y);
    }
}
fn main() int {
    let p = Point::new(3, 4);
    println(p.x);
    return 0;
}
"#);
}

#[test]
fn метод_читает_поле_через_self() {
    // `self &Point` — метод видит объект вызывающего.
    ok(r#"
struct Point { x int; y int; }
impl Point {
    fn sum(self &Point) int {
        return self.x + self.y;
    }
}
fn main() int {
    let p = Point(3, 4);
    println(p.sum());
    return 0;
}
"#);
}

#[test]
fn метод_меняет_объект_по_mut_контракту() {
    // `mut(self)` объявляет, что метод меняет объект: изменение видно
    // вызывающему.
    ok(r#"
struct Point { x int; y int; }
impl Point {
    fn shift(self &Point) mut(self) {
        self.x = self.x + 1;
    }
}
fn main() int {
    let mut p = Point(3, 4);
    p.shift();
    println(p.x);
    return 0;
}
"#);
}

#[test]
fn метод_без_mut_контракта_не_даёт_писать() {
    // Метод со `self &Point` без `mut(self)` писать не может — это то же
    // правило, что и для свободной функции.
    fails(
        r#"
struct Point { x int; y int; }
impl Point {
    fn bad(self &Point) {
        self.x = 1;
    }
}
fn main() int { return 0; }
"#,
        "mut-контракт",
    );
}

#[test]
fn вызов_мутатора_на_неизменяемой_переменной_ловится() {
    // Прямая запись `p.x = 1` в не-mut переменную запрещена. Но то же
    // изменение можно выполнить через вызов — и тогда проверка обязана
    // стоять в месте вызова, иначе неизменяемая переменная меняется в
    // обход правил. Rust запрещает это как E0596.
    fails(
        r#"
struct Point { x int; y int; }
impl Point {
    fn shift(self &Point) mut(self) { self.x = self.x + 1; }
}
fn main() int {
    let p = Point(3, 4);
    p.shift();
    return 0;
}
"#,
        "не объявлен как `mut`",
    );
}

#[test]
fn вызов_свободной_функции_мутатора_на_неизменяемой_ловится() {
    // То же для свободной функции с mut-контрактом: `shift(&p)` меняет
    // `p`, значит `p` должен быть `mut`.
    fails(
        r#"
struct Point { x int; y int; }
fn shift(p &Point) mut(p.x) { p.x = p.x + 1; }
fn main() int {
    let p = Point(3, 4);
    shift(&p);
    return 0;
}
"#,
        "не объявлен как `mut`",
    );
}

#[test]
fn вызов_мутатора_на_изменяемой_переменной_разрешён() {
    // Обратная сторона: `let mut` даёт право на вызов мутатора.
    ok(r#"
struct Point { x int; y int; }
impl Point {
    fn shift(self &Point) mut(self) { self.x = self.x + 1; }
}
fn main() int {
    let mut p = Point(3, 4);
    p.shift();
    return 0;
}
"#);
}

#[test]
fn метод_читатель_на_неизменяемой_переменной_разрешён() {
    // Читающий метод ничего не меняет, поэтому `let p` достаточно.
    ok(r#"
struct Point { x int; y int; }
impl Point {
    fn sum(self &Point) int { return self.x + self.y; }
}
fn main() int {
    let p = Point(3, 4);
    println(p.sum());
    return 0;
}
"#);
}

#[test]
fn метод_по_значению_перемещает_объект() {
    // `self Point` — объект переезжает в метод. Второй вызов невозможен.
    fails(
        r#"
struct Point { x int; y int; }
impl Point {
    fn sum(self Point) int { return self.x + self.y; }
}
fn main() int {
    let p = Point(3, 4);
    println(p.sum());
    println(p.sum());
    return 0;
}
"#,
        "перемещён",
    );
}

#[test]
fn ссылка_вместо_значения_ловится() {
    // Параметр по значению, а передан адрес. Раньше это проходило
    // проверку и падало уже в C-компиляторе, где причина не видна.
    fails(
        r#"
struct User { name string; age int; }
fn take(u User) { println(u.age); }
fn main() int {
    let u = User("John", 26);
    take(&u);
    return 0;
}
"#,
        "ожидает `User`, получен `&User`",
    );
}

#[test]
fn значение_вместо_ссылки_ловится() {
    // Обратный случай: параметр-ссылка, а передан объект.
    fails(
        r#"
struct User { name string; age int; }
fn take(u &User) { println(u.age); }
fn main() int {
    let u = User("John", 26);
    take(u);
    return 0;
}
"#,
        "ожидает ссылку",
    );
}

#[test]
fn поглощающий_метод_забирает_объект() {
    // `self User` уместен, когда метод действительно потребляет объект:
    // `into_name` возвращает поле, сам объект больше не нужен.
    ok(r#"
struct User { name string; age int; }
impl User {
    fn into_name(self User) string { return self.name; }
}
fn main() int {
    let u = User("John", 26);
    let n = u.into_name();
    println(n);
    return 0;
}
"#);
}

#[test]
fn мутация_своей_копии_не_требует_mut_у_вызывающего() {
    // Параметр по значению перемещён в функцию: его изменение
    // вызывающему не видно, поэтому `mut` у владельца не нужен.
    // Иначе `bumped(u)` требовал бы `let mut u`, хотя `u` уходит.
    ok(r#"
struct User { name string; age int; }
fn bumped(u User mut) int {
    u.age = u.age + 1;
    return u.age;
}
fn main() int {
    let u = User("John", 26);
    println(bumped(u));
    return 0;
}
"#);
}

// ============================================================
// Дубликаты имён
// ============================================================

#[test]
fn структура_объявленная_дважды_ловится() {
    fails(
        r#"
struct User { v int; }
struct User { w int; }
fn main() int { return 0; }
"#,
        "объявлена дважды",
    );
}

#[test]
fn функция_объявленная_дважды_ловится() {
    fails(
        r#"
fn f() int { return 1; }
fn f() int { return 2; }
fn main() int { return 0; }
"#,
        "объявлена дважды",
    );
}

#[test]
fn два_main_ловятся() {
    fails(
        r#"
fn main() int { return 0; }
fn main() int { return 1; }
"#,
        "`main` объявлена дважды",
    );
}

#[test]
fn метод_объявленный_дважды_ловится() {
    fails(
        r#"
struct P { x int; }
impl P {
    fn get(self &P) int { return self.x; }
    fn get(self &P) int { return 1; }
}
fn main() int { return 0; }
"#,
        "объявлен дважды",
    );
}

#[test]
fn имя_занятое_структурой_и_функцией_ловится() {
    // `User(...)` всегда означает конструктор структуры, поэтому
    // функция `User` недостижима. Раньше это молчало.
    fails(
        r#"
struct User { v int; }
fn User() int { return 1; }
fn main() int { return 0; }
"#,
        "занято и структурой, и функцией",
    );
}

#[test]
fn свободная_функция_и_метод_с_одним_именем_не_конфликтуют() {
    // Пространства имён разделены: свободная функция и метод живут
    // в разных таблицах.
    ok(r#"
struct User { v int; }
fn get() int { return 1; }
impl User {
    fn get(self &User) int { return self.v; }
}
fn main() int {
    let u = User(5);
    println(u.get());
    println(get());
    return 0;
}
"#);
}

#[test]
fn переменная_с_именем_типа_разрешена() {
    // Переменные и типы — разные пространства имён.
    ok(r#"
struct User { v int; }
fn main() int {
    let User = 5;
    println(User);
    return 0;
}
"#);
}

// ============================================================
// Импорт модулей
// ============================================================

#[test]
fn import_неизвестного_файла_ошибка() {
    // `compile` не загружает модули — он проверяет один текст.
    // Поэтому `import` в нём остаётся узлом дерева и не мешает.
    // Проверяем через файл: см. examples/modules/.
    let dir = std::env::temp_dir().join(format!("keel_imp_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let main = dir.join("main.keel");
    std::fs::write(&main, "import \"nope.keel\";\nfn main() int { return 0; }\n").unwrap();
    let r = keelc::compile_file(&main);
    assert!(r.is_err(), "ожидалась ошибка о ненайденном файле");
    let msg = r.err().unwrap();
    assert!(msg.contains("не удалось открыть"), "получено: {}", msg);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_цикл_ловится() {
    let dir = std::env::temp_dir().join(format!("keel_cyc_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("a.keel"), "import \"b.keel\";\nfn fa() int { return 1; }\n").unwrap();
    std::fs::write(dir.join("b.keel"), "import \"a.keel\";\nfn fb() int { return 2; }\n").unwrap();
    let r = keelc::compile_file(&dir.join("a.keel"));
    assert!(r.is_err(), "ожидалась ошибка о цикле");
    let msg = r.err().unwrap();
    assert!(msg.contains("цикл импорта"), "получено: {}", msg);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn диагностика_называет_файл() {
    // Ошибка в подключённом файле должна указывать его имя, а не только
    // строку: строки в разных файлах совпадают.
    let dir = std::env::temp_dir().join(format!("keel_file_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("lib.keel"),
        "pub fn bad() int {\n    let x = 5;\n    return x + true;\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.keel"),
        "import \"lib.keel\";\nfn main() int { return bad(); }\n",
    )
    .unwrap();
    let r = keelc::compile_file(&dir.join("main.keel")).expect("должно разобраться");
    let text = format_diagnostics(&r.diagnostics);
    assert!(text.contains("lib.keel:3:"), "получено:\n{}", text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn приватная_функция_не_видна_из_другого_файла() {
    let dir = std::env::temp_dir().join(format!("keel_priv_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("lib.keel"), "fn hidden() int { return 1; }\n").unwrap();
    std::fs::write(
        dir.join("main.keel"),
        "import \"lib.keel\";\nfn main() int { return hidden(); }\n",
    )
    .unwrap();
    let r = keelc::compile_file(&dir.join("main.keel")).expect("должно разобраться");
    let text = format_diagnostics(&r.diagnostics);
    assert!(text.contains("без `pub`"), "получено:\n{}", text);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn приватный_метод_не_виден_из_другого_файла() {
    let dir = std::env::temp_dir().join(format!("keel_pubimpl_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("lib.keel"),
        "pub struct S { v int; }\nimpl S {\n    fn get(self &S) int { return self.v; }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.keel"),
        "import \"lib.keel\";\nfn main() int { let s = S(7); return s.get(); }\n",
    )
    .unwrap();
    let r = keelc::compile_file(&dir.join("main.keel")).expect("должно разобраться");
    let text = format_diagnostics(&r.diagnostics);
    assert!(
        text.contains("метод `get` объявлен без `pub`"),
        "получено:\n{}",
        text
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_подключает_имена_из_другого_файла() {
    // Главная проверка: файл видит объявления из подключённого файла,
    // и сгенерированный C компилируется и работает.
    let dir = std::env::temp_dir().join(format!("keel_mod_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("user.keel"),
        "pub struct User { name string; age int; }\n\
         pub impl User {\n\
         \x20   fn get_name(self &User) string { return self.name; }\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.keel"),
        "import \"user.keel\";\n\
         fn main() int {\n\
         \x20   let u = User(\"John\", 26);\n\
         \x20   println(u.get_name());\n\
         \x20   return 0;\n\
         }\n",
    )
    .unwrap();
    let r = keelc::compile_file(&dir.join("main.keel")).expect("должно скомпилироваться");
    assert!(r.diagnostics.is_empty(), "диагностики: {:?}", r.diagnostics);
    // Метод из импортированного файла должен получить доступ к полю
    // через `->`, а не через `.` — это ловит перепутанные id выражений.
    assert!(
        r.c_code.contains("self->name"),
        "в C нет self->name:\n{}",
        r.c_code
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn один_файл_подключается_один_раз() {
    // Ромбовидный импорт: main → a, main → b, a → common, b → common.
    // `common.keel` должен загрузиться один раз, иначе структура
    // окажется объявлена дважды.
    let dir = std::env::temp_dir().join(format!("keel_dia_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("common.keel"), "pub struct Common { v int; }\n").unwrap();
    std::fs::write(
        dir.join("a.keel"),
        "import \"common.keel\";\nfn fa() int { return 1; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.keel"),
        "import \"common.keel\";\nfn fb() int { return 2; }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.keel"),
        "import \"a.keel\";\nimport \"b.keel\";\n\
         fn main() int { let c = Common(5); println(c.v); return 0; }\n",
    )
    .unwrap();
    let r = keelc::compile_file(&dir.join("main.keel")).expect("должно скомпилироваться");
    assert!(
        r.diagnostics.is_empty(),
        "структура загружена дважды: {:?}",
        r.diagnostics
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn неизвестный_метод_ловится() {
    fails(
        r#"
struct Point { x int; y int; }
fn main() int {
    let p = Point(3, 4);
    p.nope();
    return 0;
}
"#,
        "нет метода",
    );
}

#[test]
fn методы_разных_типов_с_одинаковым_именем_не_сталкиваются() {
    // В C метод получает имя `k_Тип_метод`, поэтому `Point::get` и
    // `Box::get` не конфликтуют.
    ok(r#"
struct Point { x int; }
struct Box { v int; }
impl Point {
    fn get(self &Point) int { return self.x; }
}
impl Box {
    fn get(self &Box) int { return self.v; }
}
fn main() int {
    let p = Point(1);
    let b = Box(2);
    println(p.get());
    println(b.get());
    return 0;
}
"#);
}

// ============================================================
// Волна 7: прикладной минимум
// ============================================================

#[test]
fn len_строки_и_массива() {
    ok(r#"
fn main() int {
    let s = "hello";
    let a = [1, 2, 3];
    println(len(s));
    println(len(a));
    return 0;
}
"#);
}

#[test]
fn len_работает_по_ссылке() {
    // Ссылка на массив: размер всё равно известен из типа.
    ok(r#"
fn main() int {
    let a = [1, 2, 3, 4];
    let r &[int; 4] = &a;
    println(len(r));
    return 0;
}
"#);
}

#[test]
fn len_на_числе_запрещён() {
    fails(
        r#"
fn main() int {
    let x = 5;
    println(len(x));
    return 0;
}
"#,
        "`len` работает со строкой, массивом или Vec",
    );
}

#[test]
fn len_принимает_один_аргумент() {
    fails(
        r#"
fn main() int {
    let a = [1, 2];
    println(len(a, a));
    return 0;
}
"#,
        "`len` принимает один аргумент",
    );
}

#[test]
fn печать_нескольких_значений() {
    ok(r#"
struct U { name string; age int; }
fn main() int {
    let u = U("John", 26);
    println("имя=", u.name, " возраст=", u.age);
    print("без перевода", " строки");
    println();
    return 0;
}
"#);
}

#[test]
fn println_без_аргументов_работает() {
    ok(r#"
fn main() int {
    println();
    return 0;
}
"#);
}

#[test]
fn const_числовая_подставляется() {
    ok(r#"
const MAX = 100;
fn main() int {
    println(MAX);
    let a [int; 4] = [1, 2, 3, 4];
    println(len(a));
    return 0;
}
"#);
}

#[test]
fn const_арифметика_вычисляется_при_компиляции() {
    //  — не выражение рантайма, а значение, известное компилятору.
    ok(r#"
const DOUBLE = 2 * 3;
const BIG = DOUBLE * 10;
fn main() int {
    println(DOUBLE);
    println(BIG);
    return 0;
}
"#);
}

#[test]
fn const_может_задавать_размер_массива() {
    ok(r#"
const N = 4;
fn main() int {
    let a [int; N] = [1, 2, 3, 4];
    println(len(a));
    return 0;
}
"#);
}

#[test]
fn const_строка_и_логическое() {
    ok(r#"
const GREETING = "Привет";
const FLAG = true;
fn main() int {
    println(GREETING);
    println(FLAG);
    return 0;
}
"#);
}

#[test]
fn const_из_непостоянного_значения_запрещён() {
    // Константа обязана быть известна компилятору: имя, которого нет
    // среди констант, подставить нельзя — его значение появится только
    // в рантайме.
    parse_fails(
        r#"
const Y = x;
fn main() int { return 0; }
"#,
        "не константа",
    );
}

#[test]
fn const_внутри_функции_запрещён() {
    // Константа — объявление уровня файла: у неё нет места в стеке.
    parse_fails(
        r#"
fn main() int {
    const X = 5;
    return 0;
}
"#,
        "`const` объявляется на уровне файла",
    );
}

#[test]
fn деление_на_ноль_в_константе_ловится() {
    parse_fails(
        r#"
const BAD = 5 / 0;
fn main() int { return 0; }
"#,
        "деление на ноль",
    );
}

#[test]
fn конкатенация_строк() {
    ok(r#"
fn main() int {
    let a = "Hello";
    let b = a + ", world!";
    println(b);
    return 0;
}
"#);
}

#[test]
fn конкатенация_даёт_строку() {
    // Результат `+` для строк — строка, поэтому её можно склеивать дальше
    // и брать от неё длину.
    ok(r#"
fn main() int {
    let s = "a" + "b" + "c";
    println(len(s));
    return 0;
}
"#);
}

#[test]
fn конкатенация_сравнивается_по_содержимому() {
    //  даёт новую строку в куче; сравнение — по содержимому, не по адресу.
    ok(r#"
fn main() int {
    let a = "Hello";
    if a + "!" == "Hello!" { println("равны"); }
    return 0;
}
"#);
}

#[test]
fn сложение_строки_и_числа_запрещено() {
    // Неявного приведения нет: нужен явный способ превратить число
    // в строку (пока его нет).
    fails(
        r#"
fn main() int {
    let s = "x = " + 5;
    println(s);
    return 0;
}
"#,
        "арифметика требует int",
    );
}

// ============================================================
// Найденные и исправленные баги
// ============================================================

#[test]
fn вложенный_массив_объявляется_верно() {
    // В C размеры пишутся после имени: `int64_t a[2][2]`, а не
    // `int64_t[2] a[2]`. До исправления генератор строил декларацию
    // не рекурсивно, и вложенный массив давал невалидный C.
    ok(r#"
fn main() int {
    let grid [[int; 2]; 2] = [[1, 2], [3, 4]];
    println(grid[1][0]);
    return 0;
}
"#);
}

#[test]
fn вложенный_массив_в_структуре() {
    ok(r#"
struct M { cells [[int; 2]; 2]; }
fn main() int {
    let m = M([[1, 2], [3, 4]]);
    println(m.cells[0][1]);
    return 0;
}
"#);
}

#[test]
fn вложенный_массив_в_цикле() {
    ok(r#"
fn main() int {
    let grid [[int; 3]; 2] = [[1, 2, 3], [4, 5, 6]];
    let mut sum int = 0;
    for i in 0..len(grid) {
        for j in 0..len(grid[i]) {
            sum += grid[i][j];
        }
    }
    println(sum);
    return 0;
}
"#);
}

#[test]
fn ссылка_копируется_при_печати() {
    // Ссылка — это указатель: `println(r)` не отнимает владение.
    // До исправления ссылка попадала в `is_copy` как перемещаемая,
    // и второе использование давало «`r` уже перемещён».
    ok(r#"
struct U { n int; }
fn main() int {
    let u = U(5);
    let r &U = &u;
    println(r.n);
    println(r.n);
    return 0;
}
"#);
}

#[test]
fn ссылка_на_int_используется_дважды() {
    ok(r#"
fn main() int {
    let n int = 5;
    let r &int = &n;
    println(r);
    println(r);
    return 0;
}
"#);
}

#[test]
fn ссылка_на_элемент_массива_используется_дважды() {
    ok(r#"
fn main() int {
    let a = [1, 2, 3];
    let r &int = &a[0];
    println(r);
    println(r);
    return 0;
}
"#);
}

#[test]
fn ссылка_передаётся_в_функцию_дважды() {
    ok(r#"
struct U { n int; }
fn ob(u &U) { println(u.n); }
fn main() int {
    let u = U(5);
    let r &U = &u;
    ob(r);
    ob(r);
    return 0;
}
"#);
}

#[test]
fn структура_по_прежнему_перемещается() {
    // Ссылки стали копируемыми, но структуры — нет: это по-прежнему
    // перемещение, и второе использование запрещено.
    fails(
        r#"
struct U { n int; }
fn take(u U) { }
fn main() int {
    let u = U(5);
    take(u);
    println(u.n);
    return 0;
}
"#,
        "уже перемещён",
    );
}

#[test]
fn сообщение_об_аргументах_согласовано() {
    // «ждёт 1 аргумент», а не «1 аргументов».
    fails(
        r#"
fn add(a int) int { return a; }
fn main() int {
    add(1, 2);
    return 0;
}
"#,
        "ждёт 1 аргумент, передан 2",
    );
}

#[test]
fn сообщение_о_трёх_аргументах_согласовано() {
    fails(
        r#"
fn add(a int, b int, c int) int { return a; }
fn main() int {
    add(1);
    return 0;
}
"#,
        "ждёт 3 аргумента, передан 1",
    );
}

#[test]
fn сообщение_о_нуле_аргументов_согласовано() {
    fails(
        r#"
fn f() int { return 0; }
fn main() int {
    f(1);
    return 0;
}
"#,
        "ждёт 0 аргументов, передан 1",
    );
}

// ============================================================
// Анализ выхода: ссылка не переживает кадр
// ============================================================
//
// Путь ссылки привязан к локальной переменной, а переменная умирает
// вместе с кадром. Ссылка, уехавшая в структуре, переживает кадр —
// и чтение поля после возврата даёт мусор (use-after-free).
// Компилятор C такое не ловит: он видит обычный указатель.

#[test]
fn нельзя_вернуть_ссылку_на_локальную() {
    fails(
        r#"
struct I { x int; }
fn make() &I {
    let i = I(42);
    return &i;
}
fn main() int { return 0; }
"#,
        "содержит ссылку на локальную переменную",
    );
}

#[test]
fn нельзя_вернуть_структуру_со_ссылкой_на_локальную() {
    fails(
        r#"
struct I { x int; }
struct H { r &I; }
fn make() H {
    let i = I(42);
    return H(&i);
}
fn main() int { return 0; }
"#,
        "поле `r` возвращаемого значения содержит ссылку на локальную",
    );
}

#[test]
fn нельзя_вернуть_массив_со_ссылкой_на_локальную() {
    fails(
        r#"
struct I { x int; }
fn make() [&I; 1] {
    let i = I(42);
    return [&i];
}
fn main() int { return 0; }
"#,
        "содержит ссылку на локальную переменную",
    );
}

#[test]
fn нельзя_вернуть_переменную_со_ссылкой_на_локальную() {
    // Ссылка спрятана в локальной структуре: `h` сам не ссылка,
    // но внутри него лежит ссылка на `i`.
    fails(
        r#"
struct I { x int; }
struct H { r &I; }
fn make() H {
    let i = I(42);
    let h = H(&i);
    return h;
}
fn main() int { return 0; }
"#,
        "поле `r` возвращаемого значения содержит ссылку на локальную",
    );
}

#[test]
fn нельзя_вернуть_поле_ссылающееся_на_локальную() {
    fails(
        r#"
struct I { x int; }
struct H { r &I; }
fn make() &I {
    let i = I(42);
    let h = H(&i);
    return h.r;
}
fn main() int { return 0; }
"#,
        "содержит ссылку на локальную переменную",
    );
}

#[test]
fn нельзя_вернуть_ссылку_на_поле_локальной() {
    fails(
        r#"
struct I { x int; }
fn make() &int {
    let i = I(42);
    return &i.x;
}
fn main() int { return 0; }
"#,
        "содержит ссылку на локальную переменную",
    );
}

#[test]
fn нельзя_записать_ссылку_на_локальную_в_поле_параметра() {
    // Поле параметра живёт в кадре вызывающего, который переживёт
    // наш кадр.
    fails(
        r#"
struct I { x int; }
struct H { r &I; }
fn f(h &H) mut(h.r) {
    let i = I(42);
    h.r = &i;
}
fn main() int { return 0; }
"#,
        "получает ссылку на локальную переменную",
    );
}

#[test]
fn можно_вернуть_ссылку_на_параметр() {
    // Параметр живёт в кадре вызывающего — возвращать его наружу
    // безопасно.
    ok(r#"
struct I { x int; }
fn f(x &I) &I {
    return x;
}
fn main() int { return 0; }
"#);
}

#[test]
fn можно_вернуть_структуру_со_ссылкой_на_параметр() {
    ok(r#"
struct I { x int; }
struct H { r &I; }
fn f(x &I) H {
    return H(x);
}
fn main() int { return 0; }
"#);
}

#[test]
fn можно_вернуть_ссылку_на_элемент_параметра() {
    ok(r#"
struct I { x int; }
fn f(xs &[I; 2]) &I {
    return &xs[0];
}
fn main() int { return 0; }
"#);
}

#[test]
fn можно_вернуть_обычное_значение() {
    ok(r#"
struct I { x int; }
fn make() I {
    let i = I(42);
    return i;
}
fn main() int { return 0; }
"#);
}

#[test]
fn ссылка_на_локальную_внутри_функции_разрешена() {
    // Ограничение — только на ВЫХОД за пределы функции, не на
    // использование внутри неё.
    ok(r#"
struct I { x int; }
struct H { r &I; }
fn main() int {
    let i = I(42);
    let h = H(&i);
    println(h.r.x);
    return 0;
}
"#);
}

#[test]
fn ссылка_на_локальную_в_локальном_массиве_разрешена() {
    ok(r#"
struct I { x int; }
fn main() int {
    let i = I(42);
    let a = [&i];
    println(a[0].x);
    return 0;
}
"#);
}


// ============================================================
// Волна 7 (продолжение): ввод и аргументы командной строки
// ============================================================

#[test]
fn read_line_возвращает_строку() {
    ok(r#"
fn main() int {
    let s = read_line();
    println(s);
    return 0;
}
"#);
}

#[test]
fn read_line_не_принимает_аргументов() {
    fails(
        r#"
fn main() int {
    let s = read_line(5);
    return 0;
}
"#,
        "`read_line` не принимает аргументов",
    );
}

#[test]
fn read_int_возвращает_int() {
    ok(r#"
fn main() int {
    let n = read_int();
    println(n + 1);
    return 0;
}
"#);
}

#[test]
fn read_int_не_принимает_аргументов() {
    fails(
        r#"
fn main() int {
    let n = read_int("5");
    return 0;
}
"#,
        "`read_int` не принимает аргументов",
    );
}

#[test]
fn read_int_результат_можно_сравнивать() {
    ok(r#"
fn main() int {
    let n = read_int();
    if n > 0 {
        println("положительное");
    }
    return 0;
}
"#);
}

#[test]
fn read_line_результат_это_строка() {
    // Строку из read_line можно конкатенировать и мерить.
    ok(r#"
fn main() int {
    let s = read_line();
    let t = s + "!";
    println(len(t));
    return 0;
}
"#);
}

#[test]
fn argc_возвращает_int() {
    ok(r#"
fn main() int {
    let n = argc();
    println(n);
    return 0;
}
"#);
}

#[test]
fn argc_не_принимает_аргументов() {
    fails(
        r#"
fn main() int {
    println(argc(1));
    return 0;
}
"#,
        "`argc` не принимает аргументов",
    );
}

#[test]
fn argv_возвращает_строку() {
    ok(r#"
fn main() int {
    let s = argv(0);
    println(s);
    return 0;
}
"#);
}

#[test]
fn argv_требует_один_аргумент() {
    fails(
        r#"
fn main() int {
    println(argv());
    return 0;
}
"#,
        "`argv` принимает один аргумент",
    );
}

#[test]
fn argv_требует_номер_типа_int() {
    fails(
        r#"
fn main() int {
    println(argv("0"));
    return 0;
}
"#,
        "`argv` ждёт номер аргумента типа `int`",
    );
}

#[test]
fn argv_в_цикле_по_argc() {
    ok(r#"
fn main() int {
    for i in 0..argc() {
        println(i, ": ", argv(i));
    }
    return 0;
}
"#);
}

#[test]
fn ввод_в_функции_не_только_в_main() {
    // Встроенные функции доступны в любой функции, не только в main.
    ok(r#"
fn ask() string {
    println("Имя?");
    return read_line();
}

fn main() int {
    let name = ask();
    println("Привет, ", name);
    return 0;
}
"#);
}

#[test]
fn argc_и_argv_доступны_вне_main() {
    // В C argc/argv есть только в main. В Keel — в любой функции,
    // иначе встроенные функции бесполезны для реального кода.
    ok(r#"
fn show() {
    for i in 0..argc() {
        println(argv(i));
    }
}

fn main() int {
    show();
    return 0;
}
"#);
}


// ============================================================
// return: с значением и без
// ============================================================

#[test]
fn return_без_значения_в_void_разрешён() {
    ok(r#"
fn f(x int) void {
    if x > 0 {
        return;
    }
    println("нет");
}

fn main() int {
    f(1);
    f(0);
    return 0;
}
"#);
}

#[test]
fn return_без_значения_в_int_запрещён() {
    // Раньше проходило проверку и падало на сборке C с сообщением gcc
    // «return with no value, in function returning non-void».
    fails(
        r#"
fn f() int {
    return;
}

fn main() int {
    return 0;
}
"#,
        "функция возвращает `int`, поэтому `return` требует значение",
    );
}

#[test]
fn return_без_значения_в_string_запрещён() {
    fails(
        r#"
fn f() string {
    return;
}

fn main() int {
    return 0;
}
"#,
        "функция возвращает `string`, поэтому `return` требует значение",
    );
}

#[test]
fn return_со_значением_в_void_запрещён() {
    fails(
        r#"
fn f() void {
    return 5;
}

fn main() int {
    return 0;
}
"#,
        "тип возврата: ожидался `void`, получен `int`",
    );
}

#[test]
fn return_в_main_без_значения_разрешён() {
    // main — особый случай: в C он int, но в Keel `fn main() int`
    // и `fn main()` равноправны, и голый `return;` должен работать.
    ok(r#"
fn main() int {
    println("до");
    return;
}
"#);
}

#[test]
fn ранний_return_из_цикла() {
    ok(r#"
fn find(a [int; 5], want int) int {
    for i in 0..5 {
        if a[i] == want {
            return i;
        }
    }
    return -1;
}

fn main() int {
    let a = [10, 20, 30, 40, 50];
    println(find(a, 30));
    return 0;
}
"#);
}

#[test]
fn функция_без_return_на_всех_путях() {
    // В C это UB: gcc предупреждает «control reaches end of non-void
    // function». Бэкенд дописывает возврат нуля, чтобы поведение было
    // определено. Проверка типов такое пропускает.
    ok(r#"
fn f(x int) int {
    if x > 0 {
        return 1;
    }
}

fn main() int {
    println(f(1));
    println(f(-1));
    return 0;
}
"#);
}

// ============================================================
// self — обязательный первый параметр метода
// ============================================================

#[test]
fn метод_с_self_первым_параметром() {
    ok(r#"
struct User { name string; }

impl User {
    fn get_name(self &User) string {
        return self.name;
    }
}

fn main() int {
    let u = User("Аня");
    println(u.get_name());
    return 0;
}
"#);
}

#[test]
fn метод_с_self_и_другими_параметрами() {
    ok(r#"
struct User { name string; }

impl User {
    fn greet(self &User, other string) string {
        return other;
    }
}

fn main() int {
    let u = User("Аня");
    println(u.greet("Боря"));
    return 0;
}
"#);
}

#[test]
fn self_по_значению_забирает_объект() {
    ok(r#"
struct User { name string; }

impl User {
    fn into_name(self User) string {
        return self.name;
    }
}

fn main() int {
    let u = User("Аня");
    println(u.into_name());
    return 0;
}
"#);
}

#[test]
fn self_не_первым_параметром_не_метод() {
    // `self` обязан быть первым: иначе это не метод, и вызов через
    // точку не находится.
    fails(
        r#"
struct User { name string; }

impl User {
    fn greet(other string, self &User) string {
        return other;
    }
}

fn main() int {
    let u = User("Аня");
    println(u.greet("Боря"));
    return 0;
}
"#,
        "у типа `User` нет метода `greet`",
    );
}

#[test]
fn self_без_типа_запрещён() {
    // В Rust пишут `&self`; в Keel тип обязателен — `self &User`.
    // Это ошибка разбора, а не проверки типов.
    parse_fails(
        r#"
struct User { name string; }

impl User {
    fn get_name(self) string {
        return self.name;
    }
}

fn main() int {
    return 0;
}
"#,
        "ожидалось имя",
    );
}

#[test]
fn функция_в_impl_без_self_это_ассоциированная() {
    ok(r#"
struct User { name string; }

impl User {
    fn make(n string) User {
        return User(n);
    }
}

fn main() int {
    let u = User::make("Аня");
    println(u.name);
    return 0;
}
"#);
}

#[test]
fn ассоциированную_нельзя_вызвать_через_точку() {
    fails(
        r#"
struct User { name string; }

impl User {
    fn make(n string) User {
        return User(n);
    }
}

fn main() int {
    let u = User("Аня");
    let v = u.make("Боря");
    return 0;
}
"#,
        "у типа `User` нет метода `make`",
    );
}


// ============================================================
// Вариант Б: точность модели владения
// ============================================================

// ------------------------------------------------------------
// Б.1. Move из элемента коллекции запрещён
// ------------------------------------------------------------
//
// Владелец элемента — коллекция, а не переменная. Move из элемента
// оставил бы в коллекции «дыру»: значение, которое нельзя ни
// прочитать, ни освободить. Rust такое запрещает —
// "cannot move out of index".

#[test]
fn move_из_элемента_массива_запрещён() {
    fails(
        r#"
struct S { v int; }

fn main() int {
    let a [S; 3] = [S(1), S(2), S(3)];
    let s S = a[0];
    return 0;
}
"#,
        "перемещение",
    );
}

#[test]
fn move_из_элемента_в_присваивании_запрещён() {
    // Сдвиг массива: `a[0] = a[1]` — это move из элемента a[1].
    fails(
        r#"
struct S { v int; }

fn main() int {
    let mut a [S; 3] = [S(1), S(2), S(3)];
    a[0] = a[1];
    return 0;
}
"#,
        "перемещение",
    );
}

#[test]
fn move_из_элемента_в_аргумент_запрещён() {
    fails(
        r#"
struct S { v int; }

fn take(s S) int {
    return s.v;
}

fn main() int {
    let a [S; 3] = [S(1), S(2), S(3)];
    println(take(a[0]));
    return 0;
}
"#,
        "перемещение",
    );
}

#[test]
fn move_из_элемента_в_возврате_запрещён() {
    fails(
        r#"
struct S { v int; }

fn first(a [S; 3]) S {
    return a[0];
}

fn main() int {
    return 0;
}
"#,
        "перемещение",
    );
}

#[test]
fn move_из_поля_структуры_запрещён() {
    fails(
        r#"
struct Inner { v int; }
struct Outer { inner Inner; }

fn main() int {
    let o Outer = Outer(Inner(1));
    let i Inner = o.inner;
    return 0;
}
"#,
        "перемещение",
    );
}

#[test]
fn move_из_поля_через_ссылку_запрещён() {
    // Через ссылку — тем более: поле принадлежит объекту в кадре
    // вызывающего, и забрать его нельзя.
    fails(
        r#"
struct Inner { v int; }
struct Outer { inner Inner; }

fn take(o &Outer) Inner {
    return o.inner;
}

fn main() int {
    let o Outer = Outer(Inner(1));
    let i Inner = take(&o);
    return 0;
}
"#,
        "перемещение",
    );
}

// ------------------------------------------------------------
// Б.2. Копируемые типы — по-прежнему можно
// ------------------------------------------------------------

#[test]
fn копируемый_элемент_можно_прочитать() {
    // int копируется, а не перемещается: дыры не остаётся.
    ok(r#"
fn main() int {
    let a [int; 3] = [1, 2, 3];
    let x int = a[0];
    println(x, a[1]);
    return 0;
}
"#);
}

#[test]
fn копируемое_поле_можно_прочитать() {
    ok(r#"
struct P { n int; s string; }

fn main() int {
    let p P = P(1, "два");
    let n int = p.n;
    let s string = p.s;
    println(n, s);
    return 0;
}
"#);
}

#[test]
fn сдвиг_копируемых_элементов_разрешён() {
    ok(r#"
fn main() int {
    let mut a [int; 3] = [1, 2, 3];
    a[0] = a[1];
    println(a[0]);
    return 0;
}
"#);
}

// ------------------------------------------------------------
// Б.3. Ссылка на элемент коллекции — путь элемента
// ------------------------------------------------------------
//
// Ссылка на элемент коллекции указывает на ГРУППУ элементов
// (`a[]`), а не на конкретный индекс: индекс может быть
// динамическим. Поэтому запись в элемент этой группы гасит
// ссылку — она могла указывать именно на этот элемент.
//
// До исправления: ссылка на `a[i]` (динамический индекс) переживала
// запись в `a[0]`, хотя могла указывать на него.

#[test]
fn ссылка_на_элемент_гаснет_при_записи_в_элемент() {
    // Ссылка, взятая по ДИНАМИЧЕСКОМУ индексу, указывает на группу
    // элементов `a[]`: какой именно элемент — неизвестно. Запись в
    // любой элемент этой группы гасит ссылку — она могла указывать
    // именно на этот элемент.
    fails(
        r#"
struct S { v int; }

fn main() int {
    let mut a [S; 2] = [S(1), S(2)];
    let i int = 0;
    let r &S = &a[i];
    a[0] = S(9);
    println(r.v);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn ссылка_на_элемент_гаснет_при_записи_в_другой_элемент() {
    // То же для записи в СОСЕДНИЙ элемент группы: элемент, на
    // который указывает ссылка, неизвестен — гаснет любая запись
    // в коллекцию.
    fails(
        r#"
struct S { v int; }

fn main() int {
    let mut a [S; 2] = [S(1), S(2)];
    let i int = 0;
    let r &S = &a[i];
    a[1] = S(9);
    println(r.v);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn ссылка_на_конкретный_элемент_гаснет_при_записи_в_него() {
    fails(
        r#"
struct S { v int; }

fn main() int {
    let mut a [S; 2] = [S(1), S(2)];
    let r &S = &a[0];
    a[0] = S(9);
    println(r.v);
    return 0;
}
"#,
        "инвалидирована",
    );
}

// ============================================================
// Контракты pre/post
// ============================================================

#[test]
fn контракты_pre_post_компилируются() {
    ok(r#"
fn withdraw(balance int, amount int) int
    pre(amount > 0, amount <= balance)
    post(result >= 0)
{
    return balance - amount;
}

fn main() int {
    return withdraw(100, 30);
}
"#);
}

#[test]
fn условие_pre_должно_быть_bool() {
    fails(
        r#"
fn f(x int) int pre(x) { return x; }
fn main() int { return f(1); }
"#,
        "условие `pre` должно быть bool",
    );
}

#[test]
fn условие_post_должно_быть_bool() {
    fails(
        r#"
fn f(x int) int post(result) { return x; }
fn main() int { return f(1); }
"#,
        "условие `post` должно быть bool",
    );
}

#[test]
fn result_в_void_функции_ловится() {
    fails(
        r#"
fn f(x int) post(result > 0) { print(x); }
fn main() int { f(1); return 0; }
"#,
        "`result` доступен только в post функции, возвращающей значение",
    );
}

#[test]
fn result_в_pre_неизвестен() {
    fails(
        r#"
fn f(x int) int pre(result > 0) { return x; }
fn main() int { return f(1); }
"#,
        "неизвестная переменная `result`",
    );
}

#[test]
fn повторный_контракт_pre_ловится() {
    parse_fails(
        r#"
fn f(x int) int pre(x > 0) pre(x < 10) { return x; }
fn main() int { return f(1); }
"#,
        "контракт `pre` указан дважды",
    );
}

#[test]
fn пустой_контракт_ловится() {
    parse_fails(
        r#"
fn f(x int) int pre() { return x; }
fn main() int { return f(1); }
"#,
        "контракт `pre` пуст",
    );
}

#[test]
fn post_видит_поле_result() {
    ok(r#"
struct P { x int; }
fn mk(x int) P post(result.x > 0) { return P(x); }
fn main() int { let p = mk(4); return p.x; }
"#);
}

#[test]
fn контракты_попадают_в_сгенерированный_c() {
    let r = compile(
        r#"
fn f(x int) int pre(x > 0) post(result > 0) { return x; }
fn main() int { return f(1); }
"#,
    )
    .unwrap();
    assert!(r.diagnostics.is_empty());
    assert!(r.c_code.contains("k_contract_fail"));
    assert!(r.c_code.contains("__k_result"));
}

// ============================================================
// enum и match
// ============================================================

#[test]
fn enum_и_исчерпывающий_match_компилируются() {
    ok(r#"
enum Color { Red, Green, Blue }

fn name(c Color) string {
    match c {
        Color::Red => { return "red"; }
        Color::Green => { return "green"; }
        Color::Blue => { return "blue"; }
    }
    return "?";
}

fn main() int {
    return 0;
}
"#);
}

#[test]
fn match_требует_все_варианты() {
    fails(
        r#"
enum Color { Red, Green }
fn main() int {
    let c = Color::Red;
    match c { Color::Red => { } }
    return 0;
}
"#,
        "не исчерпывающий",
    );
}

#[test]
fn match_ловит_повтор_варианта() {
    fails(
        r#"
enum Color { Red, Green }
fn main() int {
    let c = Color::Red;
    match c { Color::Red => { } Color::Red => { } Color::Green => { } }
    return 0;
}
"#,
        "разобран дважды",
    );
}

#[test]
fn match_ловит_чужой_вариант() {
    fails(
        r#"
enum Color { Red, Green }
fn main() int {
    let c = Color::Red;
    match c { Color::Red => { } Color::Green => { } Color::Blue => { } }
    return 0;
}
"#,
        "нет варианта `Blue`",
    );
}

#[test]
fn match_требует_enum() {
    fails(
        r#"
fn main() int {
    let x = 1;
    match x { A::B => { } }
    return 0;
}
"#,
        "`match` требует enum",
    );
}

#[test]
fn неизвестный_enum_в_варианте_ловится() {
    fails(
        r#"
fn main() int {
    let c = Color::Red;
    return 0;
}
"#,
        "неизвестный enum `Color`",
    );
}

#[test]
fn enum_копируется_а_не_перемещается() {
    ok(r#"
enum Color { Red, Green }
fn main() int {
    let a = Color::Red;
    let b = a;
    match a { Color::Red => { } Color::Green => { } }
    match b { Color::Red => { } Color::Green => { } }
    return 0;
}
"#);
}

#[test]
fn enum_в_поле_структуры_работает() {
    ok(r#"
enum Color { Red, Green }
struct P { c Color; }
fn main() int {
    let p = P(Color::Red);
    match p.c { Color::Red => { } Color::Green => { } }
    return 0;
}
"#);
}

#[test]
fn смена_варианта_гасит_ссылку() {
    fails(
        r#"
enum Color { Red, Green }
fn main() int {
    let mut c = Color::Red;
    let r &Color = &c;
    c = Color::Green;
    match r { Color::Red => { } Color::Green => { } }
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn enum_и_match_попадают_в_сгенерированный_c() {
    let r = compile(
        r#"
enum Color { Red, Green }
fn main() int {
    let c = Color::Red;
    match c { Color::Red => { } Color::Green => { } }
    return 0;
}
"#,
    )
    .unwrap();
    assert!(r.diagnostics.is_empty());
    assert!(r.c_code.contains("typedef enum"));
    assert!(r.c_code.contains("Keel_Color_Red"));
    assert!(r.c_code.contains("switch"));
}

// ============================================================
// Цепочка инвалидации
// ============================================================

#[test]
fn диагностика_показывает_цепочку_присваивания() {
    let r = compile(
        r#"
enum Color { Red, Green }
fn main() int {
    let mut c = Color::Red;
    let r &Color = &c;
    c = Color::Green;
    match r { Color::Red => { } Color::Green => { } }
    return 0;
}
"#,
    )
    .unwrap();
    let err = format_diagnostics(&r.diagnostics);
    assert!(err.contains("создана в строке 5"), "err = {}", err);
    assert!(
        err.contains("погашена в строке 6: присваивание"),
        "err = {}",
        err
    );
}

#[test]
fn диагностика_цепочки_называет_вызов_с_mut() {
    let r = compile(
        r#"
struct S { v int; }
fn bump(s &S) int mut(s) { s.v = s.v + 1; return s.v; }
fn main() int {
    let mut s = S(1);
    let r &int = &s.v;
    let x = bump(&s);
    println(r);
    return 0;
}
"#,
    )
    .unwrap();
    let err = format_diagnostics(&r.diagnostics);
    assert!(err.contains("создана в строке 6"), "err = {}", err);
    assert!(
        err.contains("погашена в строке 7: вызов функции с `mut(...)`"),
        "err = {}",
        err
    );
}

// ============================================================
// Варианты с данными
// ============================================================

#[test]
fn enum_с_данными_и_деструктуризация_работают() {
    ok(r#"
enum Opt { None, Some(int) }
fn get(o Opt) int {
    match o {
        Opt::None => { return -1; }
        Opt::Some(x) => { return x; }
    }
    return -1;
}
fn main() int {
    let a = Opt::Some(42);
    let b = Opt::None;
    return get(a) + get(b);
}
"#);
}

#[test]
fn вариант_с_данными_требует_вызова() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some;
    return 0;
}
"#,
        "несёт данные: нужен вызов",
    );
}

#[test]
fn match_ловит_неверное_число_привязок() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some(1);
    match o { Opt::None => { } Opt::Some(x, y) => { } }
    return 0;
}
"#,
        "в ветви привязано 2",
    );
}

#[test]
fn match_ловит_привязки_у_варианта_без_данных() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some(1);
    match o { Opt::None(x) => { } Opt::Some(y) => { } }
    return 0;
}
"#,
        "в ветви привязано 1",
    );
}

#[test]
fn данные_варианта_проверяются_по_типу() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some("x");
    return 0;
}
"#,
        "ожидался `int`, получен `string`",
    );
}

#[test]
fn некопируемый_тип_в_данных_отклоняется() {
    fails(
        r#"
struct S { v int; }
enum E { A(S) }
fn main() int { return 0; }
"#,
        "в данных пока не поддерживается",
    );
}

#[test]
fn рекурсивный_enum_отклоняется() {
    fails(
        r#"
enum E { A(E) }
fn main() int { return 0; }
"#,
        "рекурсивен",
    );
}

#[test]
fn match_по_ссылке_на_enum_с_данными() {
    ok(r#"
enum Opt { None, Some(int) }
fn get(o &Opt) int {
    match o {
        Opt::None => { return 0; }
        Opt::Some(x) => { return x; }
    }
    return 0;
}
fn main() int {
    let o = Opt::Some(7);
    return get(&o);
}
"#);
}

#[test]
fn enum_с_данными_в_поле_структуры() {
    ok(r#"
enum Opt { None, Some(int) }
struct H { o Opt; }
fn main() int {
    let h = H(Opt::Some(5));
    match h.o {
        Opt::None => { return 0; }
        Opt::Some(x) => { return x; }
    }
    return 0;
}
"#);
}

#[test]
fn enum_с_данными_попадает_в_c_как_tagged_union() {
    let r = compile(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some(1);
    match o { Opt::None => { } Opt::Some(x) => { } }
    return 0;
}
"#,
    )
    .unwrap();
    assert!(r.diagnostics.is_empty());
    assert!(r.c_code.contains("Keel_Opt_tag"));
    assert!(r.c_code.contains("union"));
    assert!(r.c_code.contains(".data.Some"));
}

#[test]
fn сравнение_enum_с_данными_отклоняется() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some(1);
    if o == Opt::Some(1) { return 1; }
    return 0;
}
"#,
        "сравнение enum с данными не поддерживается",
    );
}

#[test]
fn печать_enum_с_данными_отклоняется() {
    fails(
        r#"
enum Opt { None, Some(int) }
fn main() int {
    let o = Opt::Some(1);
    println(o);
    return 0;
}
"#,
        "печать enum с данными не поддерживается",
    );
}

// ============================================================
// Option / Result — встроенные обобщённые enum'ы
// ============================================================

#[test]
fn option_конструирование_и_match() {
    ok(
        r#"
fn find(xs [int; 3], target int) Option<int> {
    let mut i int = 0;
    while i < 3 {
        if xs[i] == target { return Some(i); }
        i = i + 1;
    }
    return None;
}

fn main() int {
    let xs [int; 3] = [10, 20, 30];
    let o Option<int> = find(xs, 20);
    match o {
        Some(i) => { println(i); }
        None => { println(-1); }
    }
    return 0;
}
"#,
    );
}

#[test]
fn none_без_типа_ошибка() {
    fails(
        r#"
fn main() int {
    let o = None;
    return 0;
}
"#,
        "не удалось вывести тип `None`",
    );
}

#[test]
fn ok_без_типа_ошибки_ошибка() {
    fails(
        r#"
fn main() int {
    let r = Ok(5);
    return 0;
}
"#,
        "не удалось вывести тип ошибки `Ok`",
    );
}

#[test]
fn option_неисчерпывающий_match() {
    fails(
        r#"
fn main() int {
    let o Option<int> = Some(1);
    match o {
        Some(x) => { println(x); }
    }
    return 0;
}
"#,
        "не разобраны варианты `Option::None`",
    );
}

#[test]
fn option_неверный_вариант() {
    fails(
        r#"
fn main() int {
    let o Option<int> = Some(1);
    match o {
        Ok(x) => { println(x); }
        Err(e) => { println(e); }
    }
    return 0;
}
"#,
        "не относится к enum `Option`",
    );
}

#[test]
fn option_в_c_как_tagged_union() {
    let r = compile(
        r#"
fn main() int {
    let o Option<int> = Some(5);
    match o {
        Some(x) => { println(x); }
        None => { println(0); }
    }
    return 0;
}
"#,
    )
    .unwrap();
    assert!(r.diagnostics.is_empty());
    assert!(r.c_code.contains("Keel_Option_int_tag"));
    assert!(r.c_code.contains("Keel_Option_int_Some"));
    assert!(r.c_code.contains("Keel_Option_int_None"));
    assert!(r.c_code.contains(".data.Some"));
}

#[test]
fn result_ok_err() {
    ok(
        r#"
fn div(a int, b int) Result<int, string> {
    if b == 0 { return Err("деление на ноль"); }
    return Ok(a / b);
}

fn main() int {
    let r Result<int, string> = div(10, 2);
    match r {
        Ok(v) => { println(v); }
        Err(e) => { println(e); }
    }
    return 0;
}
"#,
    );
}

#[test]
fn option_печать_запрещена() {
    fails(
        r#"
fn main() int {
    let o Option<int> = Some(1);
    println(o);
    return 0;
}
"#,
        "печать enum с данными не поддерживается",
    );
}

#[test]
fn option_сравнение_запрещено() {
    fails(
        r#"
fn main() int {
    let o Option<int> = Some(1);
    if o == o { return 1; }
    return 0;
}
"#,
        "сравнение enum с данными не поддерживается",
    );
}

#[test]
fn option_параметр_выводит_none() {
    ok(
        r#"
fn take(o Option<int>) int {
    match o {
        Some(x) => { return x; }
        None => { return -1; }
    }
}

fn main() int {
    let x = take(None);
    println(x);
    return 0;
}
"#,
    );
}

#[test]
fn option_в_option() {
    ok(
        r#"
fn main() int {
    let o Option<Option<int>> = Some(None);
    match o {
        Some(inner) => {
            match inner {
                Some(x) => { println(x); }
                None => { println(0); }
            }
        }
        None => { println(-1); }
    }
    return 0;
}
"#,
    );
}

#[test]
fn option_в_поле_и_массиве() {
    ok(
        r#"
struct Holder { o Option<int>; }

fn main() int {
    let h Holder = Holder(None);
    match h.o {
        Some(x) => { println(x); }
        None => { println(0); }
    }
    let arr [Option<int>; 2] = [Some(1), None];
    let mut i int = 0;
    while i < 2 {
        match arr[i] {
            Some(x) => { println(x); }
            None => { println(-1); }
        }
        i = i + 1;
    }
    return 0;
}
"#,
    );
}

// ============================================================
// Vec<T>
// ============================================================

#[test]
fn vec_push_len_get_pop() {
    ok(
        r#"
fn main() int {
    let mut v Vec<int> = Vec::new();
    v.push(1);
    v.push(2);
    println(v.len());
    println(v[0]);
    v[1] = 9;
    match v.get(1) {
        Some(x) => { println(x); }
        None => { println(0); }
    }
    match v.pop() {
        Some(x) => { println(x); }
        None => { println(0); }
    }
    v.free();
    return 0;
}
"#,
    );
}

#[test]
fn vec_без_типа_ошибка() {
    fails(
        r#"
fn main() int {
    let v = Vec::new();
    return 0;
}
"#,
        "не удалось вывести тип `Vec::new`",
    );
}

#[test]
fn vec_push_в_неизменяемый_запрещён() {
    fails(
        r#"
fn main() int {
    let v Vec<int> = Vec::new();
    v.push(1);
    return 0;
}
"#,
        "получатель должен быть `mut`",
    );
}

#[test]
fn vec_push_гасит_ссылку_на_элемент() {
    fails(
        r#"
fn main() int {
    let mut v Vec<int> = Vec::new();
    v.push(1);
    let r = &v[0];
    v.push(2);
    println(r);
    return 0;
}
"#,
        "инвалидирована",
    );
}

#[test]
fn vec_печать_запрещена() {
    fails(
        r#"
fn main() int {
    let mut v Vec<int> = Vec::new();
    v.push(1);
    println(v);
    return 0;
}
"#,
        "печать Vec не поддерживается",
    );
}

#[test]
fn vec_сравнение_запрещено() {
    fails(
        r#"
fn main() int {
    let mut a Vec<int> = Vec::new();
    let mut b Vec<int> = Vec::new();
    if a == b { return 1; }
    return 0;
}
"#,
        "сравнение Vec не поддерживается",
    );
}

#[test]
fn vec_вложенный_и_структура() {
    ok(
        r#"
struct P { x int; }

fn main() int {
    let mut ps Vec<P> = Vec::new();
    ps.push(P(1));
    ps.push(P(2));
    println(ps[1].x);

    let mut grid Vec<Vec<int>> = Vec::new();
    let mut row Vec<int> = Vec::new();
    row.push(7);
    grid.push(row);
    println(grid[0][0]);
    return 0;
}
"#,
    );
}

#[test]
fn vec_после_free_нельзя_использовать() {
    fails(
        r#"
fn main() int {
    let mut v Vec<int> = Vec::new();
    v.push(1);
    v.free();
    println(v.len());
    return 0;
}
"#,
        "перемещён",
    );
}

// ============================================================
// Именованный конструктор структуры
// ============================================================

#[test]
fn именованный_конструктор_работает() {
    ok(r#"
struct Point { x int; y int; }

fn main() int {
    let p = Point { x: 3, y: 4 };
    println(p.x);
    return 0;
}
"#);
}

#[test]
fn именованный_конструктор_без_поля() {
    fails(
        r#"
struct Point { x int; y int; }

fn main() int {
    let p = Point { x: 3 };
    return 0;
}
"#,
        "не задано поле `y`",
    );
}

#[test]
fn именованный_конструктор_чужое_поле() {
    fails(
        r#"
struct Point { x int; }

fn main() int {
    let p = Point { x: 3, z: 4 };
    return 0;
}
"#,
        "нет поля `z`",
    );
}

#[test]
fn именованный_конструктор_тип_поля() {
    fails(
        r#"
struct Point { x int; }

fn main() int {
    let p = Point { x: true };
    return 0;
}
"#,
        "поле `x` ожидает `int`",
    );
}

#[test]
fn именованный_конструктор_повтор_поля() {
    fails(
        r#"
struct Point { x int; }

fn main() int {
    let p = Point { x: 1, x: 2 };
    return 0;
}
"#,
        "поле `x` задано дважды",
    );
}

// ============================================================
// Кортежи
// ============================================================

#[test]
fn кортеж_работает() {
    ok(r#"
fn main() int {
    let t = (1, true);
    println(t.0);
    println(t.1);
    return 0;
}
"#);
}

#[test]
fn кортеж_в_функции() {
    ok(r#"
fn pair() (int, bool) {
    return (7, true);
}

fn main() int {
    let t = pair();
    println(t.0);
    return 0;
}
"#);
}

#[test]
fn кортеж_копируется() {
    ok(r#"
fn main() int {
    let a = (1, 2);
    let b = a;
    println(a.0);
    println(b.0);
    return 0;
}
"#);
}

#[test]
fn кортеж_индекс_за_границей() {
    fails(
        r#"
fn main() int {
    let t = (1, 2);
    println(t.5);
    return 0;
}
"#,
        "нет элемента `5`",
    );
}

// ============================================================
// float (f64)
// ============================================================

#[test]
fn float_арифметика() {
    ok(r#"
fn main() int {
    let a float = 3.5;
    let b float = 2.0;
    println(a + b);
    println(a * b);
    println(a / b);
    println(a > b);
    return 0;
}
"#);
}

#[test]
fn float_смешанная_арифметика() {
    fails(
        r#"
fn main() int {
    let a float = 1.0;
    let b int = 2;
    println(a + b);
    return 0;
}
"#,
        "арифметика требует int",
    );
}

// ============================================================
// Срезы [T]
// ============================================================

#[test]
fn срез_из_массива() {
    ok(r#"
fn main() int {
    let arr [int; 3] = [10, 20, 30];
    let s [int] = arr;
    println(len(s));
    println(s[1]);
    return 0;
}
"#);
}

#[test]
fn срез_в_функции() {
    ok(r#"
fn sum(xs [int]) int {
    let mut s int = 0;
    let mut i int = 0;
    while i < len(xs) {
        s = s + xs[i];
        i = i + 1;
    }
    return s;
}

fn main() int {
    let arr [int; 3] = [10, 20, 30];
    println(sum(arr));
    return 0;
}
"#);
}

#[test]
fn срез_не_перемещает_массив() {
    ok(r#"
fn main() int {
    let arr [int; 2] = [1, 2];
    let s [int] = arr;
    println(arr[0]);
    println(s[0]);
    return 0;
}
"#);
}

// ============================================================
// Тело-выражение
// ============================================================

#[test]
fn тело_выражение() {
    ok(r#"
fn add(a int, b int) int {
    a + b
}

fn main() int {
    println(add(2, 3));
    return 0;
}
"#);
}

#[test]
fn тело_выражение_if() {
    ok(r#"
fn max(a int, b int) int {
    if a > b { a } else { b }
}

fn main() int {
    println(max(7, 4));
    return 0;
}
"#);
}

// ============================================================
// Замыкания
// ============================================================

#[test]
fn замыкание_вызов() {
    ok(r#"
fn main() int {
    let inc fn(int) -> int = |x int| x + 1;
    println(inc(5));
    return 0;
}
"#);
}

#[test]
fn замыкание_в_функции() {
    ok(r#"
fn apply(f fn(int) -> int, x int) int {
    f(x)
}

fn main() int {
    let inc fn(int) -> int = |x int| x + 1;
    println(apply(inc, 10));
    return 0;
}
"#);
}

#[test]
fn замыкание_возврат_из_функции() {
    ok(r#"
fn make_adder() fn(int) -> int {
    |x int| x + 1
}

fn main() int {
    let f fn(int) -> int = make_adder();
    println(f(5));
    return 0;
}
"#);
}
