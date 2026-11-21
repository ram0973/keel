/* ============================================================
 *  Keel — рантайм: собственный аллокатор
 * ============================================================
 *
 *  Сгенерированный C вызывает malloc/realloc/free из libc напрямую.
 *  Здесь — свои функции: ke_alloc, ke_realloc, ke_free. Зачем:
 *
 *  1. Счётчики живых блоков. На выходе из программы рантайм сообщает,
 *     сколько памяти не освобождено. Для языка, чья главная ценность —
 *     безопасность памяти, это должно быть видно, а не молчать.
 *  2. Проверка магического числа в заголовке блока: освобождение
 *     мусорного указателя и двойное освобождение ловятся, а не
 *     молча портят кучу.
 *  3. Место, где появится свой аллокатор без libc (WASM, freestanding)
 *     и нумерация поколений для Gen<T>.
 *
 *  Разметка блока:
 *
 *      +------------------+------------------------+
 *      | Ke_Block (заголовок) | полезные байты ...  |
 *      | size | magic       |                     |
 *      +------------------+------------------------+
 *      ^ указатель, который видит программа
 *
 *  Файл подставляется в сгенерированный C целиком: out.c остаётся
 *  самодостаточным, его можно собрать без исходников Keel.
 * ============================================================ */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* «KEEL» — magic, чтобы отличить наш блок от чужого указателя. */
#define KE_MAGIC 0x4B45454C4C45534Bull

typedef struct Ke_Block {
    size_t size;    /* размер полезной части */
    uint64_t magic; /* KE_MAGIC, если блок наш */
} Ke_Block;

static size_t ke_live_blocks = 0; /* живых блоков */
static size_t ke_live_bytes = 0;  /* живых байт */

/* Отчёт об утечках — при выходе из программы. */
static void ke_report_leaks(void) {
    if (ke_live_blocks != 0) {
        fprintf(stderr,
                "keel: утечка памяти — не освобождено блоков: %zu, байт: %zu\n",
                ke_live_blocks, ke_live_bytes);
    }
}

/* Регистрируем отчёт один раз — при первом обращении к аллокатору. */
static void ke_init(void) {
    static int done = 0;
    if (!done) {
        done = 1;
        atexit(ke_report_leaks);
    }
}

/* Выделить блок не меньше size байт. */
void *ke_alloc(size_t size) {
    ke_init();
    Ke_Block *b = (Ke_Block *)malloc(sizeof(Ke_Block) + (size ? size : 1));
    if (b == NULL) {
        fprintf(stderr, "keel: не хватило памяти (%zu байт)\n", size);
        exit(1);
    }
    b->size = size;
    b->magic = KE_MAGIC;
    ke_live_blocks++;
    ke_live_bytes += size;
    return (void *)(b + 1);
}

/* Освободить блок. Мусорный указатель — ошибка, а не порча кучи. */
void ke_free(void *p) {
    if (p == NULL) {
        return;
    }
    Ke_Block *b = (Ke_Block *)p - 1;
    if (b->magic != KE_MAGIC) {
        fprintf(stderr, "keel: освобождение мусорного указателя\n");
        exit(1);
    }
    b->magic = 0;
    ke_live_blocks--;
    ke_live_bytes -= b->size;
    free(b);
}

/* Выделить блок и скопировать в него содержимое.
 * Нужно для `Box::new(x)`: значение кладётся в кучу целиком. */
void *ke_alloc_copy(size_t size, const void *src) {
    void *p = ke_alloc(size);
    memcpy(p, src, size);
    return p;
}

/* Изменить размер блока, сохранив содержимое. */
void *ke_realloc(void *p, size_t size) {
    if (p == NULL) {
        return ke_alloc(size);
    }
    Ke_Block *b = (Ke_Block *)p - 1;
    if (b->magic != KE_MAGIC) {
        fprintf(stderr, "keel: изменение размера мусорного указателя\n");
        exit(1);
    }
    size_t old = b->size;
    if (size == old) {
        return p;
    }
    Ke_Block *nb = (Ke_Block *)realloc(b, sizeof(Ke_Block) + (size ? size : 1));
    if (nb == NULL) {
        fprintf(stderr, "keel: не хватило памяти (%zu байт)\n", size);
        exit(1);
    }
    nb->size = size;
    ke_live_bytes -= old;
    ke_live_bytes += size;
    return (void *)(nb + 1);
}

/* Сколько блоков и байт живёт прямо сейчас. Для тестов и диагностики. */
size_t ke_live_blocks_count(void) { return ke_live_blocks; }
size_t ke_live_bytes_count(void) { return ke_live_bytes; }
