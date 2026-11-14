#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// Нарушение контракта pre/post: сообщение и выход с кодом 1.
static void k_contract_fail(const char* fn, const char* kind, int line) {
  fprintf(stderr, "нарушен контракт %s функции `%s` (строка %d)\n", kind, fn, line);
  exit(1);
}

// Проверка границ индексации.
static int64_t k_index_check(int64_t i, int64_t len, int line) {
  if (i < 0 || i >= len) {
    fprintf(stderr, "строка %d: индекс %lld вне границ [0, %lld)\n", line, (long long)i, (long long)len);
    exit(1);
  }
  return i;
}

// Читает строку из stdin до '\n' (сам '\n' не входит).
// Возвращает NULL в конце ввода (EOF).
// Память не освобождается — см. волну 8 в docs/lang-design.md.
static char* k_read_line(void) {
  size_t cap = 64, len = 0;
  char* buf = (char*)malloc(cap);
  if (!buf) return NULL;
  int c;
  while ((c = getchar()) != EOF && c != '\n') {
    if (len + 1 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
    buf[len++] = (char)c;
  }
  if (c == EOF && len == 0) { free(buf); return NULL; }
  buf[len] = '\0';
  return buf;
}

// Читает целое из stdin. Читает СТРОКУ и разбирает её —
// иначе перевод строки остался бы в буфере ввода
// и сломал бы следующий read_line.
static int64_t k_read_int(void) {
  char* s = k_read_line();
  if (!s) return 0;
  return (int64_t)strtoll(s, NULL, 10);
}

// То же, но NULL заменяется пустой строкой: в языке нет
// Option, а печатать NULL нельзя.
static const char* k_read_line_or_empty(void) {
  char* s = k_read_line();
  return s ? s : "";
}

// Аргументы командной строки. main() кладёт их сюда, чтобы
// argc()/argv(i) работали в ЛЮБОЙ функции, а не только в main.
static int64_t k_argc = 0;
static char** k_argv = 0;

static int64_t k_open(const char* path, const char* mode) {
  FILE* f = fopen(path, mode);
  return f ? (int64_t)(intptr_t)f : -1;
}
static char* k_read(int64_t fd) {
  FILE* f = (FILE*)(intptr_t)fd;
  if (!f) return "";
  size_t cap = 256, len = 0;
  char* buf = (char*)malloc(cap);
  if (!buf) return "";
  for (;;) {
    if (len + 1 >= cap) { cap *= 2; buf = (char*)realloc(buf, cap); }
    size_t n = fread(buf + len, 1, cap - len - 1, f);
    len += n;
    if (n == 0) break;
  }
  buf[len] = '\0';
  return buf;
}
static void k_write(int64_t fd, const char* s) {
  FILE* f = (FILE*)(intptr_t)fd;
  if (f) fputs(s, f);
}
static void k_close(int64_t fd) {
  FILE* f = (FILE*)(intptr_t)fd;
  if (f) fclose(f);
}

// Склеивает две строки в новый буфер из кучи.
// Освобождения пока нет — см. волну 8 в docs/lang-design.md.
static char* k_str_concat(const char* a, const char* b) {
  size_t la = strlen(a), lb = strlen(b);
  char* r = (char*)malloc(la + lb + 1);
  memcpy(r, a, la);
  memcpy(r + la, b, lb + 1);
  return r;
}




int main(int __k_argc, char** __k_argv) {
  k_argc = __k_argc;
  k_argv = __k_argv;
#line 2 "shadow.keel"
  int64_t printf = INT64_C(1);
#line 3 "shadow.keel"
  printf("%lld\n", (long long)printf);
#line 4 "shadow.keel"
  return INT64_C(0);
  return 0;
}

