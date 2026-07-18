#ifndef RSSRT_H
#define RSSRT_H

#define RSSRT_ABI_VERSION 1

/*
 * Stage 4 bootstrap runtime ABI.
 *
 * This header deliberately contains only the scalar output operation needed
 * by rss-ir-v1's initial `fn main() -> Int` contract. It is portable C11 and
 * has no Rust or Cargo dependency.
 */

void rssrt_print_int(long long value);

#endif
