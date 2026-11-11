// Тот же бенчмарк, что и examples/bench.keel, на Rust.
// Нужен как эталон для scripts/bench.py.

fn fib(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    fib(n - 1) + fib(n - 2)
}

fn main() {
    let mut sum: i64 = 0;
    let mut i: i64 = 0;
    while i < 100_000_000 {
        sum = sum + i % 7;
        i = i + 1;
    }
    println!("{}", sum);
    println!("{}", fib(30));
}
