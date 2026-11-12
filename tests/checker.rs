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
