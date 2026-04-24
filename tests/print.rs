fn print_wat(src: &str) -> String {
    let wasm = rwat::parse_rwat(src).unwrap();
    wasmprinter::print_bytes(&wasm).unwrap()
}

#[test]
fn test_print_plain_module() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (func)
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (func (;0;) (type 0))
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_import_only_symbol_module() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (import "env" "foo" (func $foo (@sym) (type 0)))
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env" "foo" (func $foo (;0;) (type 0)))
  (@custom "linking" (after import) "\02\08\04\01\00\10\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_table_import_symbol_module() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (import "env" "tab" (table $tab 1 externref))
            )
        "#,
    );
    let expected = r#"(module
  (import "env" "tab" (table $tab (;0;) 1 externref))
  (@custom "linking" (after import) "\02\08\04\01\05\10\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_defined_symbol_module_without_imports() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (func $foo (@sym) (type 0))
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (func $foo (;0;) (type 0))
  (@custom "linking" (after code) "\02\08\08\01\00\00\00\03foo")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_empty_module() {
    let actual = print_wat(
        r#"
            (module (@rwat))
        "#,
    );
    assert_eq!(actual.trim_end(), "(module)");
}

#[test]
fn test_print_explicit_symbols_and_return_call_reloc() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (import "env" "foo" (func $foo (@sym (name "foo.sym"))))
              (func $bar (@sym (name "bar.sym"))
                return_call $foo (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env" "foo" (func $foo (;0;) (type 0)))
  (func $bar (;1;) (type 0)
    return_call $foo
  )
  (@custom "linking" (after code) "\02\08\17\02\00P\00\07foo.sym\00\00\01\07bar.sym")
  (@custom "reloc.CODE" (after code) "\03\01\00\04\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_multiple_symbols_and_relocs() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func (param i32) (result i32)))
              (type (func (param i32) (result externref)))
              (type (func (param externref) (result i32)))
              (import "env" "__linear_memory" (memory 0))
              (import "string" "test"
                (func $string.test.import (@sym) (type 1)))
              (import "env" "js_sys.externref.insert"
                (func $js_sys.externref.insert (@sym) (type 2)))
              (func $string_test (@sym (name "string.test")) (type 0)
                local.get 0
                call $string.test.import (@reloc)
                call $js_sys.externref.insert (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func (param i32) (result i32)))
  (type (;1;) (func (param i32) (result externref)))
  (type (;2;) (func (param externref) (result i32)))
  (import "env" "__linear_memory" (memory (;0;) 0))
  (import "string" "test" (func $string.test.import (;0;) (type 1)))
  (import "env" "js_sys.externref.insert" (func $js_sys.externref.insert (;1;) (type 2)))
  (func $string_test (;2;) (type 0) (param i32) (result i32)
    local.get 0
    call $string.test.import
    call $js_sys.externref.insert
  )
  (@custom "linking" (after code) "\02\08\16\03\00\10\00\00\10\01\00\00\02\0bstring.test")
  (@custom "reloc.CODE" (after code) "\03\02\00\06\00\00\0c\01")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_group1_imports() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (import "env"
                (item "foo" (func $foo (@sym)))
                (item "mem" (memory 0)))
              (func $bar (@sym)
                call $foo (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env"
    (item "foo" (func $foo (;0;) (type 0)))
    (item "mem" (memory (;0;) 0))
  )
  (func $bar (;1;) (type 0)
    call $foo
  )
  (@custom "linking" (after code) "\02\08\0b\02\00\10\00\00\00\01\03bar")
  (@custom "reloc.CODE" (after code) "\03\01\00\04\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_group2_imports() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (import "env"
                (item "foo")
                (func))
              (func $bar (@sym)
                call 0 (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env"
    (item "foo")
    (func (type 0))
  )
  (func $bar (;1;) (type 0)
    call 0
  )
  (@custom "linking" (after code) "\02\08\0b\02\00\10\00\00\00\01\03bar")
  (@custom "reloc.CODE" (after code) "\03\01\00\04\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_call_indirect_table_reloc() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (type (func))
              (import "env" "tab" (table $tab 1 funcref))
              (func
                i32.const 0
                call_indirect $tab (type 0) (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env" "tab" (table $tab (;0;) 1 funcref))
  (func (;0;) (type 0)
    i32.const 0
    call_indirect (type 0)
  )
  (@custom "linking" (after code) "\02\08\04\01\05\10\00")
  (@custom "reloc.CODE" (after code) "\03\01\14\07\00")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_print_table_copy_relocates_both_table_immediates() {
    let actual = print_wat(
        r#"
            (module (@rwat)
              (import "env" "dst" (table $dst 1 externref))
              (import "env" "src" (table $src 1 externref))
              (func
                table.copy $dst $src (@reloc)
              )
            )
        "#,
    );
    let expected = r#"(module
  (type (;0;) (func))
  (import "env" "dst" (table $dst (;0;) 1 externref))
  (import "env" "src" (table $src (;1;) 1 externref))
  (func (;0;) (type 0)
    table.copy $dst $src
  )
  (@custom "linking" (after code) "\02\08\07\02\05\10\00\05\10\01")
  (@custom "reloc.CODE" (after code) "\03\02\14\05\00\14\0a\01")
)
"#;
    assert_eq!(actual, expected);
}

#[test]
fn test_parse_rejects_missing_rwat_annotation() {
    let wat = r#"
            (module
              (func)
            )
        "#;

    let err = rwat::parse_rwat(wat).unwrap_err().to_string();
    assert!(err.contains("expected module header annotation `(@rwat)`"));
}

#[test]
fn test_parse_rejects_anonymous_defined_function_symbol() {
    let wat = r#"
            (module (@rwat)
              (type (func))
              (func (@sym) (type 0))
            )
        "#;

    let err = rwat::parse_rwat(wat).unwrap_err().to_string();
    assert!(err.contains(
        "defined function symbols require an explicit `@sym (name ...)` or function identifier",
    ));
}

#[test]
fn test_parse_rejects_reloc_to_unnamed_defined_function() {
    let wat = r#"
            (module (@rwat)
              (type (func))
              (func (type 0))
              (func $caller (type 0)
                call 0 (@reloc)
              )
            )
        "#;

    let err = rwat::parse_rwat(wat).unwrap_err().to_string();
    assert!(err.contains(
        "defined function symbols require an explicit `@sym (name ...)` or function identifier",
    ));
}

#[test]
fn test_parse_rejects_reloc_to_unnamed_defined_table() {
    let wat = r#"
            (module (@rwat)
              (type (func))
              (table 1 funcref)
              (func
                i32.const 0
                call_indirect 0 (type 0) (@reloc)
              )
            )
        "#;

    let err = rwat::parse_rwat(wat).unwrap_err().to_string();
    assert!(err.contains(
        "defined table symbols require an explicit `@sym (name ...)` or table identifier",
    ));
}
