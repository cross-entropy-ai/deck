# vt100 0.16.2 local patch

## Bug: `Row::clear_wide` out-of-bounds panic

`Row::clear_wide(col)` accesses `cells[col + 1]` without bounds check when
`cells[col].is_wide()` is true. When `col` is the last column index
(`cols - 1`), this accesses `cells[cols]` — one past the end of the vec.

```
thread 'main' panicked at vt100-0.16.2/src/row.rs:89:28:
index out of bounds: the len is 130 but the index is 130
```

### Trigger path

`Screen::el` (CSI K, erase line) -> `Grid::erase_row_forward` ->
iterates `pos.col..size.cols` -> `Row::erase(cols-1)` ->
`Row::clear_wide(cols-1)` -> accesses `cells[cols]` -> panic

### How a wide char ends up at the last column

`Row::resize()` (used by `Grid::set_size`) calls `Vec::resize` to shrink
rows but does NOT clear a wide char whose continuation cell got truncated.
Compare with `Row::truncate()` which explicitly handles this case. After
a resize that splits a wide char at the boundary, the last cell remains
marked `is_wide()` with no valid continuation — triggering the OOB on
the next erase.

### Fix applied

1. **`Row::clear_wide`**: bounds check before accessing `col + 1`;
   if out of range, clear the orphaned wide cell in-place.
2. **`Row::resize`**: when shrinking, check the new last cell for
   `is_wide()` and clear it (matching what `truncate` already does).

### Upstream status

vt100 0.16.2 is the latest release (2025-07-12). The bug exists on
the `main` branch of `doy/vt100-rust` as of 2026-04.
