use crate::annotation;
use crate::code::{encode_reloc_code_section, patch_code_section};
use crate::emit::{emit_module, raw_sections};
use crate::link::{build_link_info, linking_section};
use crate::scan;
use crate::types::{FuncAnnotation, ParsedRelocFunc, RelocImports, RelocWat, TableAnnotation};
use wast::core::{Func, FuncKind, Imports, Module, ModuleField, ModuleKind, Table};
use wast::parser::{self, Parse, ParseBuffer, Parser};
use wast::token::{Id, NameAnnotation, Span};
use wast::{Wat, kw};

/// Re-export `wast`'s `Error`
pub type Error = wast::Error;

/// A convenience type definition for `Result` where the error is hardwired to
/// [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Parses annotated wat into relocatable wasm file.
///
/// The input must use the `rwat` extensions, including the required
/// module-level `(@rwat)` annotation. On success, the returned bytes contain
/// the encoded wasm module plus the generated `linking` and `reloc.CODE`
/// custom sections.
pub fn parse_rwat(wat: &str) -> Result<Vec<u8>> {
    let mut buf = ParseBuffer::new(wat)?;
    buf.track_instr_spans(true);
    let rwat = parser::parse::<RelocWat>(&buf)?;

    let mut buf = ParseBuffer::new(wat)?;
    buf.track_instr_spans(true);
    let mut parsed = parser::parse::<Wat>(&buf)?;
    let module = parse_text_module(wat, &mut parsed)?;

    let link_info = {
        let ModuleKind::Text(fields) = &module.kind else {
            unreachable!("checked");
        };
        build_link_info(wat, &rwat, fields)?
    };
    let wasm = module.encode()?;

    let sections = raw_sections(wasm.as_slice());
    let patched_code = patch_code_section(wasm.as_slice(), &sections, &link_info.defined_funcs)?;
    let linking = linking_section(&link_info);
    let reloc = encode_reloc_code_section(&link_info, patched_code.as_ref());

    Ok(emit_module(wasm, sections, patched_code, linking, reloc))
}

fn parse_text_module<'a>(wat: &'a str, parsed: &'a mut Wat<'a>) -> Result<&'a mut Module<'a>> {
    let module = match parsed {
        Wat::Module(module) => module,
        Wat::Component(_) => {
            return Err(error(
                wat,
                Span::from_offset(0),
                "expected a core wasm module",
            ));
        }
    };

    let ModuleKind::Text(fields) = &module.kind else {
        return Err(error(wat, module.span, "binary modules are not supported"));
    };

    for field in fields {
        let ModuleField::Custom(custom) = field else {
            continue;
        };
        if custom.name() == "linking" || custom.name().starts_with("reloc.") {
            return Err(error(
                wat,
                module.span,
                "input wat already defines `linking`/`reloc.*` custom sections, but also uses `@sym`/`@reloc`; this combination is not supported yet",
            ));
        }
    }

    module.resolve()?;

    Ok(module)
}

impl<'a> Parse<'a> for RelocWat<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let mut rwat = RelocWat::default();
        let _rwat = parser.register_annotation("rwat");
        if parser.peek2::<kw::module>()? {
            parser.parens(|parser| {
                let module_span = parser.parse::<kw::module>()?.0;
                if parser.peek2::<annotation::rwat>()? {
                    parser.parens(|parser| parser.parse::<annotation::rwat>())?;
                } else {
                    return Err(
                        parser.error_at(module_span, "expected module header annotation `(@rwat)`")
                    );
                }
                let _: Option<Id<'a>> = parser.parse()?;
                let _: Option<NameAnnotation<'a>> = parser.parse()?;

                while !parser.is_empty() {
                    if parser.peek2::<kw::import>()? {
                        rwat.import_annotations
                            .push(parser.parens(RelocImports::parse)?);
                    } else if parser.peek2::<kw::func>()? {
                        match parser.parens(ParsedRelocFunc::parse)? {
                            ParsedRelocFunc::Import(sym) => rwat
                                .import_annotations
                                .push(RelocImports { syms: vec![sym] }),
                            ParsedRelocFunc::Defined(annotation) => {
                                rwat.func_annotations.push(annotation)
                            }
                        }
                    } else if parser.peek2::<kw::table>()? {
                        rwat.table_annotations
                            .push(parser.parens(TableAnnotation::parse)?);
                    } else {
                        parser.parens(|parser| parser.parse::<ModuleField<'a>>().map(|_| ()))?;
                    }
                }
                Ok(())
            })?;
        }
        Ok(rwat)
    }
}

impl<'a> Parse<'a> for RelocImports<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let syms = {
            let _sym = parser.register_annotation("sym");
            parser.step(|cursor| {
                let start = cursor;
                let (syms, _) = scan::scan_import_syms(cursor)?;
                Ok((syms, start))
            })?
        };
        let _: Imports<'a> = parser.parse()?;
        Ok(RelocImports { syms })
    }
}

impl<'a> Parse<'a> for ParsedRelocFunc<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let sym = {
            let _sym = parser.register_annotation("sym");
            parser.step(|cursor| {
                let start = cursor;
                let (sym, _) = scan::scan_func_sym(cursor)?;
                Ok((sym, start))
            })?
        };
        let reloc_spans = {
            let _reloc = parser.register_annotation("reloc");
            parser.step(|cursor| {
                let start = cursor;
                let (reloc_spans, _) = scan::scan_func_reloc_spans(cursor)?;
                Ok((reloc_spans, start))
            })?
        };
        let func: Func<'a> = parser.parse()?;
        match func.kind {
            FuncKind::Import(_, _) => {
                if !reloc_spans.is_empty() {
                    return Err(parser.error_at(
                        func.span,
                        "`@reloc` is only allowed in inline function bodies",
                    ));
                }
                Ok(ParsedRelocFunc::Import(sym))
            }
            FuncKind::Inline { .. } => Ok(ParsedRelocFunc::Defined(FuncAnnotation {
                sym,
                reloc_spans,
            })),
        }
    }
}

impl<'a> Parse<'a> for TableAnnotation<'a> {
    fn parse(parser: Parser<'a>) -> Result<Self> {
        let sym = {
            let _sym = parser.register_annotation("sym");
            parser.step(|cursor| {
                let start = cursor;
                let (sym, _) = scan::scan_table_sym(cursor)?;
                Ok((sym, start))
            })?
        };
        let _: Table<'a> = parser.parse()?;
        Ok(TableAnnotation { sym })
    }
}

pub(crate) fn error(wat: &str, span: Span, msg: impl Into<String>) -> wast::Error {
    let mut err = wast::Error::new(span, msg.into());
    err.set_text(wat);
    err
}
