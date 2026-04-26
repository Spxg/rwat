# rwat

`rwat` means `reloc wat`: it parses annotated wat into a wasm binary while automatically emitting `linking` and `reloc.CODE` custom sections.

The main entry point is:

```rust
pub fn parse_rwat(wat: &str) -> wast::parser::Result<Vec<u8>>
```

## Annotations

`rwat` extends plain wat with three annotations:

- `(@rwat)`: required on the module header to enable `rwat` parsing.
- `(@sym)` or `(@sym (name "..."))`: declares a symbol for a function/table import or function/table definition.
- `(@reloc)`: marks the immediately preceding relocatable instruction as requiring a relocation entry. This includes `call`, `return_call`, `call_indirect`, `return_call_indirect`, and table instructions such as `table.get`, `table.copy`, and `table.size`.

For function or table definitions, if you write `(@sym)` without an explicit name, `rwat` uses the item ID as the symbol name when available.

Currently, `rwat` only emits two WebAssembly relocation types: `R_WASM_FUNCTION_INDEX_LEB` for function indices and `R_WASM_TABLE_NUMBER_LEB` for table indices.

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
    | 5. patch function/table immediates
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

- `wast` does not understand `(@sym)` and `(@reloc)` as first-class syntax, because these annotations are custom to `rwat` rather than part of the official wat grammar.
- The parser/encoder integration points needed to preserve annotation metadata through encoding are mostly private APIs, so an external crate cannot directly plug this behavior into `wast`.
- getting such changes accepted upstream in `wast` would be difficult,
- carrying a private `wast` fork would create ongoing maintenance cost.

## Example

The [examples/add](examples/add) directory builds two wat files separately, then links them with `wasm-ld`: `add.wat` defines the `add` symbol, and `main.wat` imports it, marks the call as relocatable, and defines `main(a, b)`.

```sh
cargo run -- examples/add/add.wat -o add.o
cargo run -- examples/add/main.wat -o main.o
wasm-ld --no-entry --export=main main.o add.o -o main.wasm
# 42
wasmtime --invoke main main.wasm 20 22
```
