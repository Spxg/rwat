# rwat

`rwat` stands for `Reloc WAT`, for turning annotated text WAT into a wasm binary, while automatically emitting `linking` and `reloc.CODE` custom sections.

The main entry point is:

```rust
pub fn parse_rwat(wat: &str) -> wast::parser::Result<Vec<u8>>
```

## Annotations

`rwat` extends plain WAT with three annotations:

- `(@rwat)`: required on the module header to enable `rwat` parsing.
- `(@sym)` or `(@sym (name "..."))`: declares a symbol for a function/table import or function/table definition.
- `(@reloc)`: marks the immediately preceding relocatable instruction as requiring a relocation entry. This includes `call`, `return_call`, `call_indirect`, `return_call_indirect`, and table instructions such as `table.get`, `table.copy`, and `table.size`.

For function or table definitions, if you write `(@sym)` without an explicit name, `rwat` uses the item ID as the symbol name when available.

## How It Works

At a high level, `rwat` uses `wast` for text parsing and normal wasm emission, `wasmparser` for reading back section/operator offsets, and `wasm-encoder` for assembling the final output:

```text
annotated WAT
    |
    | 1. scan custom annotations
    |    - (@rwat)
    |    - (@sym)
    |    - (@reloc)
    v
custom annotation metadata
    +
    | 2. parse the same source as normal WAT with `wast`
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

## Why Not Patch `wast`?

In principle, modifying `wast` directly would make this implementation simpler. This project does not take that route for a practical reason: these annotations are custom syntax, not part of the official WAT grammar and not tied to a standardized proposal. Because of that:

- `wast` exposes a fair amount of parser/encoder functionality only through private APIs, so an external crate cannot directly reuse some of the integration points that would make this approach practical,
- getting such changes accepted upstream in `wast` would be difficult,
- carrying a private `wast` fork would create ongoing maintenance cost.

## Example

Given this WAT:

```wat
(module (@rwat)
  (type (func))
  (import "env" "foo" (func $foo (@sym) (type 0)))
  (func $bar (@sym (name "bar.sym")) (type 0)
    call $foo (@reloc)
  )
)
```

The generated wasm keeps the normal code section and also includes:

- `linking`: symbol table metadata.
- `reloc.CODE`: relocation records for function indices and table immediates in the code section.

```text
wat.o:	file format wasm 0x1

Section Details:

Type[1]:
 - type[0] () -> nil
Import[1]:
 - func[0] sig=0 <foo> <- env.foo
Function[1]:
 - func[1] sig=0 <bar>
Code[1]:
 - func[1] size=8 <bar>
Custom:
 - name: "linking"
  - symbol table [count=2]
   - 0: F <foo> func=0 [ undefined binding=global vis=default ]
   - 1: F <bar.sym> func=1 [ binding=global vis=default ]
Custom:
 - name: "reloc.CODE"
  - relocations for section: 3 (Code) [1]
   - R_WASM_FUNCTION_INDEX_LEB offset=0x000004(file=0x000025) symbol=0 <foo>
Custom:
 - name: "name"
 - func[0] <foo>
 - func[1] <bar>
```

For fuller input/output examples, see [tests/print.rs](tests/print.rs).
