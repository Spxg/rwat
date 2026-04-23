use wast::parser::Cursor;
use wast::parser::Result;
use wast::token::Span;

// Example:
// `(func $f (@sym (name "foo")) (result i32) i32.const 0)`
//  -> `Some(Some("foo"))`
// `(func $f (@sym) (result i32) i32.const 0)`
//  -> `Some(None)`
// `(func $f (result i32) i32.const 0)`
//  -> `None`
pub(crate) fn scan_func_sym<'a>(
    cursor: Cursor<'a>,
) -> Result<(Option<Option<&'a str>>, Cursor<'a>)> {
    let cursor = expect_keyword(cursor, "func")?;
    let cursor = if let Some((_, next)) = cursor.id()? {
        next
    } else {
        cursor
    };
    scan_sym_annotation(cursor)
}

// Example:
// `(import "env" "f" (func $f (@sym (name "foo"))))`
//  -> `[Some(Some("foo"))]`
// `(import "env" (item "f" (func $f (@sym))) (item "g" (memory 1)))`
//  -> `[Some(None), None]`
// `(import "env" (item "f") (item "g") (func))`
//  -> `[None, None]`
pub(crate) fn scan_import_syms<'a>(
    cursor: Cursor<'a>,
) -> Result<(Vec<Option<Option<&'a str>>>, Cursor<'a>)> {
    let cursor = expect_keyword(cursor, "import")?;
    let (_, cursor) = expect_string(cursor)?;

    if let Some((_, cursor)) = cursor.string()? {
        let (sym, cursor) = scan_item_sig(cursor)?;
        return Ok((vec![sym], cursor));
    }

    let mut cursor = cursor;
    let mut item_syms = Vec::new();
    while peek_group_item(cursor)? {
        let (has_sig, sym, next) = scan_group_item(cursor)?;
        item_syms.push((has_sig, sym));
        cursor = next;
    }

    if item_syms.is_empty() {
        return Err(cursor.error("expected a string or import items"));
    }

    if item_syms.iter().all(|(has_sig, _)| *has_sig) {
        return Ok((item_syms.into_iter().map(|(_, sym)| sym).collect(), cursor));
    }

    if item_syms.iter().all(|(has_sig, _)| !*has_sig) {
        let (sym, cursor) = scan_item_sig(cursor)?;
        if sym.is_some() {
            return Err(cursor.error("`@sym` is not allowed on group2 imports"));
        }
        return Ok((vec![None; item_syms.len()], cursor));
    }

    Err(cursor.error("unexpected `)`"))
}

// Example:
// `(func
//    call $foo (@reloc)
//    i32.const 0
//    return_call $bar (@reloc)
//  )`
//  -> spans for the `call` and `return_call` instructions
pub(crate) fn scan_func_reloc_spans<'a>(mut cursor: Cursor<'a>) -> Result<(Vec<Span>, Cursor<'a>)> {
    let mut relocs = Vec::new();
    let mut last_top_level_call = None;

    loop {
        if let Some((keyword, next)) = cursor.keyword()? {
            last_top_level_call =
                matches!(keyword, "call" | "return_call").then(|| cursor.cur_span());
            cursor = next;
            continue;
        }

        if let Some(next) = cursor.lparen()? {
            if let Some((annotation, after_annotation)) = next.annotation()?
                && annotation == "reloc"
            {
                let Some(call_span) = last_top_level_call else {
                    return Err(cursor.error("`@reloc` must follow `call` or `return_call`"));
                };
                relocs.push(call_span);
                cursor = expect_rparen(after_annotation)?;
                continue;
            }

            cursor = skip_until_rparen(next)?;
            continue;
        }

        if let Some(next) = cursor.rparen()? {
            cursor = next;
            break;
        }

        if let Some(next) = advance_cursor(cursor)? {
            cursor = next;
            continue;
        }

        return Err(cursor.error("unexpected token"));
    }

    Ok((relocs, cursor))
}

fn peek_group_item(cursor: Cursor<'_>) -> Result<bool> {
    let Some(cursor) = cursor.lparen()? else {
        return Ok(false);
    };
    let Some((keyword, _)) = cursor.keyword()? else {
        return Ok(false);
    };
    Ok(keyword == "item")
}

fn scan_group_item<'a>(cursor: Cursor<'a>) -> Result<(bool, Option<Option<&'a str>>, Cursor<'a>)> {
    let cursor = expect_lparen(cursor)?;
    let cursor = expect_keyword(cursor, "item")?;
    let (_, cursor) = expect_string(cursor)?;

    if let Some(cursor) = cursor.rparen()? {
        return Ok((false, None, cursor));
    }

    let (sym, cursor) = scan_item_sig(cursor)?;
    let cursor = expect_rparen(cursor)?;
    Ok((true, sym, cursor))
}

// Example:
// `(func $f (@sym (name "foo")) (type 0))`
//  -> `Some(Some("foo"))`
// `(func $f (@sym) (type 0))`
//  -> `Some(None)`
// `(memory 1)`
//  -> `None`
fn scan_item_sig<'a>(cursor: Cursor<'a>) -> Result<(Option<Option<&'a str>>, Cursor<'a>)> {
    let cursor = expect_lparen(cursor)?;
    let Some((kind, mut cursor)) = cursor.keyword()? else {
        return Err(cursor.error("expected an import type"));
    };

    let mut sym = None;
    if kind == "func" {
        if let Some((_, next)) = cursor.id()? {
            cursor = next;
        }
        let (next_sym, next) = scan_sym_annotation(cursor)?;
        sym = next_sym;
        cursor = next;
    }

    cursor = skip_until_rparen(cursor)?;
    Ok((sym, cursor))
}

// Example:
// `(@sym (name "foo"))` -> `Some(Some("foo"))`
// `(@sym)` -> `Some(None)`
// `(type 0)` -> `None`
fn scan_sym_annotation<'a>(cursor: Cursor<'a>) -> Result<(Option<Option<&'a str>>, Cursor<'a>)> {
    let start = cursor;
    let Some(cursor) = cursor.lparen()? else {
        return Ok((None, cursor));
    };
    let Some((annotation, cursor)) = cursor.annotation()? else {
        return Ok((None, start));
    };
    if annotation != "sym" {
        return Ok((None, start));
    }

    let (name, cursor) = if let Some(cursor) = cursor.rparen()? {
        return Ok((Some(None), cursor));
    } else {
        let cursor = expect_lparen(cursor)?;
        let cursor = expect_keyword(cursor, "name")?;
        let (name, cursor) = expect_utf8_string(cursor)?;
        let cursor = expect_rparen(cursor)?;
        let cursor = expect_rparen(cursor)?;
        (Some(name), cursor)
    };

    Ok((Some(name), cursor))
}

fn skip_until_rparen<'a>(mut cursor: Cursor<'a>) -> Result<Cursor<'a>> {
    loop {
        if let Some(cursor2) = cursor.rparen()? {
            return Ok(cursor2);
        }
        if let Some(cursor2) = cursor.lparen()? {
            cursor = skip_until_rparen(cursor2)?;
            continue;
        }
        if let Some((_, cursor2)) = cursor.id()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.keyword()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.reserved()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.integer()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.float()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.string()? {
            cursor = cursor2;
            continue;
        }
        if let Some((_, cursor2)) = cursor.annotation()? {
            cursor = cursor2;
            continue;
        }
        return Err(cursor.error("unexpected token"));
    }
}

fn advance_cursor<'a>(cursor: Cursor<'a>) -> Result<Option<Cursor<'a>>> {
    if let Some((_, next)) = cursor.annotation()? {
        return Ok(Some(next));
    }
    if let Some((_, next)) = cursor.id()? {
        return Ok(Some(next));
    }
    if let Some((_, next)) = cursor.reserved()? {
        return Ok(Some(next));
    }
    if let Some((_, next)) = cursor.integer()? {
        return Ok(Some(next));
    }
    if let Some((_, next)) = cursor.float()? {
        return Ok(Some(next));
    }
    if let Some((_, next)) = cursor.string()? {
        return Ok(Some(next));
    }
    Ok(None)
}

fn expect_keyword<'a>(cursor: Cursor<'a>, expected: &str) -> Result<Cursor<'a>> {
    let Some((keyword, cursor)) = cursor.keyword()? else {
        return Err(cursor.error(format!("expected `{expected}`")));
    };
    if keyword == expected {
        Ok(cursor)
    } else {
        Err(cursor.error(format!("expected `{expected}`")))
    }
}

fn expect_lparen<'a>(cursor: Cursor<'a>) -> Result<Cursor<'a>> {
    cursor.lparen()?.ok_or_else(|| cursor.error("expected `(`"))
}

fn expect_rparen<'a>(cursor: Cursor<'a>) -> Result<Cursor<'a>> {
    cursor.rparen()?.ok_or_else(|| cursor.error("expected `)`"))
}

fn expect_string<'a>(cursor: Cursor<'a>) -> Result<(&'a [u8], Cursor<'a>)> {
    cursor
        .string()?
        .ok_or_else(|| cursor.error("expected a string"))
}

fn expect_utf8_string<'a>(cursor: Cursor<'a>) -> Result<(&'a str, Cursor<'a>)> {
    let (bytes, cursor) = expect_string(cursor)?;
    let value = std::str::from_utf8(bytes).map_err(|_| cursor.error("expected a utf-8 string"))?;
    Ok((value, cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wast::parser;
    use wast::parser::ParseBuffer;
    use wast::parser::Parse;
    use wast::parser::Parser;

    fn into_owned(sym: Option<Option<&str>>) -> Option<Option<String>> {
        sym.map(|name| name.map(str::to_owned))
    }

    fn parse_func_sym(src: &str) -> Option<Option<String>> {
        let buf = ParseBuffer::new(src).unwrap();
        into_owned(parser::parse::<FuncSym<'_>>(&buf).unwrap().0)
    }

    fn parse_import_syms(src: &str) -> Vec<Option<Option<String>>> {
        let buf = ParseBuffer::new(src).unwrap();
        parser::parse::<ImportSyms<'_>>(&buf)
            .unwrap()
            .0
            .into_iter()
            .map(into_owned)
            .collect()
    }

    fn parse_import_syms_err(src: &str) -> String {
        let buf = ParseBuffer::new(src).unwrap();
        match parser::parse::<ImportSyms<'_>>(&buf) {
            Ok(_) => panic!("expected parse error"),
            Err(err) => err.to_string(),
        }
    }

    fn parse_reloc_count(src: &str) -> usize {
        let buf = ParseBuffer::new(src).unwrap();
        parser::parse::<RelocSpans>(&buf).unwrap().0.len()
    }

    fn parse_reloc_err(src: &str) -> String {
        let buf = ParseBuffer::new(src).unwrap();
        match parser::parse::<RelocSpans>(&buf) {
            Ok(_) => panic!("expected parse error"),
            Err(err) => err.to_string(),
        }
    }

    fn parse_item_sig(src: &str) -> Option<Option<String>> {
        let buf = ParseBuffer::new(src).unwrap();
        into_owned(parser::parse::<ItemSig<'_>>(&buf).unwrap().0)
    }

    fn parse_sym_annotation(src: &str) -> Option<Option<String>> {
        let buf = ParseBuffer::new(src).unwrap();
        into_owned(parser::parse::<SymAnnotation<'_>>(&buf).unwrap().0)
    }

    struct FuncSym<'a>(Option<Option<&'a str>>);

    impl<'a> Parse<'a> for FuncSym<'a> {
        fn parse(parser: Parser<'a>) -> Result<Self> {
            let _sym = parser.register_annotation("sym");
            parser.parens(|parser| {
                let sym = parser.step(scan_func_sym)?;
                Ok(FuncSym(sym))
            })
        }
    }

    struct ImportSyms<'a>(Vec<Option<Option<&'a str>>>);

    impl<'a> Parse<'a> for ImportSyms<'a> {
        fn parse(parser: Parser<'a>) -> Result<Self> {
            let _sym = parser.register_annotation("sym");
            parser.parens(|parser| {
                let syms = parser.step(scan_import_syms)?;
                Ok(ImportSyms(syms))
            })
        }
    }

    struct RelocSpans(Vec<Span>);

    impl<'a> Parse<'a> for RelocSpans {
        fn parse(parser: Parser<'a>) -> Result<Self> {
            let _reloc = parser.register_annotation("reloc");
            let spans = parser.step(|cursor| {
                let cursor = expect_lparen(cursor)?;
                scan_func_reloc_spans(cursor)
            })?;
            Ok(RelocSpans(spans))
        }
    }

    struct ItemSig<'a>(Option<Option<&'a str>>);

    impl<'a> Parse<'a> for ItemSig<'a> {
        fn parse(parser: Parser<'a>) -> Result<Self> {
            let _sym = parser.register_annotation("sym");
            let sym = parser.step(scan_item_sig)?;
            Ok(ItemSig(sym))
        }
    }

    struct SymAnnotation<'a>(Option<Option<&'a str>>);

    impl<'a> Parse<'a> for SymAnnotation<'a> {
        fn parse(parser: Parser<'a>) -> Result<Self> {
            let _sym = parser.register_annotation("sym");
            let sym = parser.step(|cursor| {
                let (sym, cursor) = scan_sym_annotation(cursor)?;
                let cursor = if sym.is_none() {
                    let cursor = expect_lparen(cursor)?;
                    skip_until_rparen(cursor)?
                } else {
                    cursor
                };
                Ok((sym, cursor))
            })?;
            Ok(SymAnnotation(sym))
        }
    }

    #[test]
    fn test_scan_func_sym_examples() {
        assert_eq!(parse_func_sym(r#"(func $f (@sym (name "foo")))"#), Some(Some("foo".into())));
        assert_eq!(parse_func_sym(r#"(func $f (@sym))"#), Some(None));
        assert_eq!(parse_func_sym(r#"(func $f)"#), None);
    }

    #[test]
    fn test_scan_import_syms_examples() {
        assert_eq!(
            parse_import_syms(r#"(import "env" "f" (func $f (@sym (name "foo"))))"#),
            vec![Some(Some("foo".into()))],
        );
        assert_eq!(
            parse_import_syms(r#"(import "env" (item "f" (func $f (@sym))) (item "g" (memory 1)))"#),
            vec![Some(None), None],
        );
        assert_eq!(
            parse_import_syms(r#"(import "env" (item "f") (item "g") (func))"#),
            vec![None, None],
        );
    }

    #[test]
    fn test_scan_import_syms_rejects_group2_sym() {
        let err = parse_import_syms_err(r#"(import "env" (item "f") (func (@sym)))"#);
        assert!(err.contains("`@sym` is not allowed on group2 imports"));
    }

    #[test]
    fn test_scan_func_reloc_spans_examples() {
        assert_eq!(
            parse_reloc_count(r#"(func call $foo (@reloc) i32.const 0 return_call $bar (@reloc))"#),
            2,
        );
    }

    #[test]
    fn test_scan_func_reloc_spans_rejects_non_call_annotation() {
        let err = parse_reloc_err(r#"(func i32.const 0 (@reloc))"#);
        assert!(err.contains("`@reloc` must follow `call` or `return_call`"));
    }

    #[test]
    fn test_scan_item_sig_examples() {
        assert_eq!(parse_item_sig(r#"(func $f (@sym (name "foo")) (type 0))"#), Some(Some("foo".into())));
        assert_eq!(parse_item_sig(r#"(func $f (@sym) (type 0))"#), Some(None));
        assert_eq!(parse_item_sig(r#"(memory 1)"#), None);
    }

    #[test]
    fn test_scan_sym_annotation_examples() {
        assert_eq!(parse_sym_annotation(r#"(@sym (name "foo"))"#), Some(Some("foo".into())));
        assert_eq!(parse_sym_annotation(r#"(@sym)"#), Some(None));
        assert_eq!(parse_sym_annotation(r#"(type 0)"#), None);
    }
}
