// Рантайм Cranelift-бэкенда: печать значений.
//
// Бэкенд компилирует Keel в машинный код, поэтому всё, что не помещается
// в IR, живёт здесь. Состав — минимальный: пять функций печати.

#include <stdint.h>
#include <stdio.h>

void ke_clif_print_i64(int64_t v) { printf("%lld", (long long)v); }
void ke_clif_print_f64(double v) { printf("%g", v); }
void ke_clif_print_str(const char *s) { fputs(s, stdout); }
void ke_clif_print_bool(int64_t v) { fputs(v ? "true" : "false", stdout); }
void ke_clif_print_newline(void) { fputc('\n', stdout); }
