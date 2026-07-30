# rwat

[![Crates.io](https://img.shields.io/crates/v/rwat.svg)](https://crates.io/crates/rwat)

`rwat` means `reloc wat`: it parses annotated wat into a wasm binary while automatically emitting `linking` and `reloc.CODE` custom sections.

The main entry point is:

```rust
pub fn parse_rwat(wat: &str) -> wast::parser::Result<Vec<u8>>
```

## Annotations

`rwat` extends plain wat with three annotations:

- `(@rwat)`: required on the module header to enable `rwat` parsing.
- `(@sym)` or `(@sym (name "..."))`: declares a symbol for a function, table, or tag import or definition.
- `(@reloc)`: marks the immediately preceding relocatable instruction as requiring a relocation entry. This includes `call`, `return_call`, `call_indirect`, `return_call_indirect`, `throw`, typed `try_table` catches, and table instructions such as `table.get`, `table.copy`, and `table.size`.

For function, table, or tag definitions, if you write `(@sym)` without an explicit name, `rwat` uses the item ID as the symbol name when available.

Currently, `rwat` emits `R_WASM_FUNCTION_INDEX_LEB` for function indices, `R_WASM_TYPE_INDEX_LEB` for indirect-call type indices, `R_WASM_TAG_INDEX_LEB` for tag indices, and `R_WASM_TABLE_NUMBER_LEB` for table indices.

## How It Works

At a high level, `rwat` uses `wast` for text parsing and normal wasm emission, `wasmparser` for reading back section/operator offsets, and `wasm-encoder` for assembling the final output:

```text
annotated wat
    |
    | 1. scan custom annotations
    |    - (@rwat)
    |    - (@sym)
    |    - (@reloc)
    v
custom annotation metadata
    +
    | 2. parse the same source as normal wat with `wast`
    v
wast AST / resolved module
    |
    | 3. `wast` encodes the module
    v
plain wasm bytes
    |
    | 4. `wasmparser` reads raw sections
    |    and decodes the code section
    v
code section + relocatable-immediate offsets
    |
    | 5. patch function/type/table/tag immediates
    |    to fixed-width 5-byte LEBs when `(@reloc)` is present
    |    so relocation offsets stay stable
    v
patched code section
    |
    | 6. emit `linking` symbol table
    |    and `reloc.CODE` entries
    v
`wasm-encoder` final assembly
    |
    v
final wasm object bytes
```

## Why Not Use `wast` Directly

`rwat` still uses `wast` for standard wat parsing and encoding, but it cannot rely on `wast` alone for this extension:

- `(@sym)` and `(@reloc)` are `rwat` annotations, not part of the official wat grammar, so `wast` does not parse them as first-class syntax.
- Preserving those annotations through encoding would require parser and encoder integration points that are mostly private in `wast`.
- Upstreaming this behavior would be difficult, and maintaining a private `wast` fork would add long-term maintenance cost.

## Example

The [examples](examples) directory builds two wat files and links them with `lld`: `add.wat` defines the `add` symbol, and `main.wat` imports it, marks the call as relocatable, and defines `main(a, b)`.

```sh
cargo install rwat --locked
rwat examples/main.wat examples/add.wat -o main.wasm -Wl,--no-entry,--export=main
# 42
wasmtime --invoke main main.wasm 20 22
```
