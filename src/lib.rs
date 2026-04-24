use std::collections::{HashMap, HashSet};
use std::ops::Range;

use wasm_encoder::CustomSection;
use wasm_encoder::Encode;
use wasm_encoder::LinkingSection;
use wasm_encoder::Module as WasmModule;
use wasm_encoder::RawSection as WasmRawSection;
use wasm_encoder::SymbolTable;
use wasmparser::CodeSectionReader;
use wasmparser::Operator;
use wasmparser::Parser as WasmParser;
use wasmparser::{BinaryReader, FunctionBody};
use wast::core::FuncKind;
use wast::core::Imports;
use wast::core::ItemKind;
use wast::core::ModuleField;
use wast::core::ModuleKind;
use wast::core::Table;
use wast::core::TableKind;
use wast::core::{Func, Instruction};
use wast::kw;
use wast::parser;
use wast::parser::Parse;
use wast::parser::ParseBuffer;
use wast::parser::Parser;
use wast::parser::Result;
use wast::token::Id;
use wast::token::Index;
use wast::token::NameAnnotation;
use wast::token::Span;

mod scan;

mod annotation {
    wast::annotation!(rwat);
}

/// code section's id always 10
const CODE_SECTION_ID: u8 = 10;
const RELOC_FUNCTION_INDEX_LEB: u8 = 0;
const RELOC_TABLE_NUMBER_LEB: u8 = 20;

#[derive(Debug, Default)]
struct RelocWat<'a> {
    import_annotations: Vec<RelocImports<'a>>,
    func_annotations: Vec<FuncAnnotation<'a>>,
    table_annotations: Vec<TableAnnotation<'a>>,
}

#[derive(Debug)]
struct RelocImports<'a> {
    syms: Vec<Option<Option<&'a str>>>,
}

#[derive(Debug)]
struct FuncAnnotation<'a> {
    sym: Option<Option<&'a str>>,
    reloc_spans: Vec<Span>,
}

#[derive(Debug)]
struct TableAnnotation<'a> {
    sym: Option<Option<&'a str>>,
}

#[derive(Debug)]
enum ParsedRelocFunc<'a> {
    Import(Option<Option<&'a str>>),
    Defined(FuncAnnotation<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SymbolKey {
    Function(u32),
    Table(u32),
}

#[derive(Debug)]
enum Symbol<'a> {
    FunctionImport {
        index: u32,
        explicit_name: Option<&'a str>,
    },
    FunctionDefined {
        index: u32,
        symbol_name: &'a str,
    },
    TableImport {
        index: u32,
        explicit_name: Option<&'a str>,
    },
    TableDefined {
        index: u32,
        symbol_name: &'a str,
    },
}

#[derive(Debug)]
struct DefinedFunc<'a> {
    symbol_name: Option<&'a str>,
    reloc_instrs: Vec<ParsedRelocInstruction>,
    span: Span,
}

#[derive(Debug)]
struct DefinedTable<'a> {
    symbol_name: Option<&'a str>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedRelocInstruction {
    has_reloc: bool,
    targets: Vec<SymbolKey>,
}

#[derive(Debug)]
struct LinkInfo<'a> {
    symbols: Vec<Symbol<'a>>,
    symbol_indices: HashMap<SymbolKey, u32>,
    defined_funcs: Vec<DefinedFunc<'a>>,
}

#[derive(Debug)]
struct CodeRelocation {
    offset: u32,
    reloc_type: u8,
    target: SymbolKey,
}

#[derive(Debug)]
struct PatchedCodeSection {
    section_index: u32,
    payload: Vec<u8>,
    relocations: Vec<CodeRelocation>,
}

#[derive(Debug, Clone)]
struct RawSection {
    id: u8,
    index: u32,
    range: Range<usize>,
}

/// Parses annotated WAT source into a wasm binary.
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
    let mut parsed = parser::parse::<wast::Wat>(&buf)?;
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

fn parse_text_module<'a>(
    wat: &'a str,
    parsed: &'a mut wast::Wat<'a>,
) -> Result<&'a mut wast::core::Module<'a>> {
    let module = match parsed {
        wast::Wat::Module(module) => module,
        wast::Wat::Component(_) => {
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
                "input WAT already defines `linking`/`reloc.*` custom sections, but also uses `@sym`/`@reloc`; this combination is not supported yet",
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

fn resolve_func_relocs(
    func: &Func<'_>,
    reloc_spans: &[Span],
    linking: &mut HashSet<SymbolKey>,
) -> Vec<ParsedRelocInstruction> {
    let FuncKind::Inline { expression, .. } = &func.kind else {
        return Vec::new();
    };

    let instr_spans = match (reloc_spans.is_empty(), expression.instr_spans.as_deref()) {
        (true, _) => None,
        (false, Some(instr_spans)) => Some(instr_spans),
        (false, None) => panic!("expected instruction spans when resolving `@reloc` metadata"),
    };

    let mut reloc_iter = reloc_spans.iter().copied().peekable();
    let mut reloc_instrs = Vec::new();

    for (i, instr) in expression.instrs.iter().enumerate() {
        let Some(targets) = reloc_instruction_targets(instr) else {
            continue;
        };

        let mut has_reloc = false;
        if let Some(instr_spans) = instr_spans {
            while reloc_iter.next_if(|span| *span == instr_spans[i]).is_some() {
                has_reloc = true;
            }
        }
        if has_reloc {
            linking.extend(targets.iter().copied());
        }

        reloc_instrs.push(ParsedRelocInstruction { has_reloc, targets });
    }

    assert!(
        reloc_iter.next().is_none(),
        "expected every `@reloc` to match a relocatable instruction",
    );

    reloc_instrs
}

fn reloc_instruction_targets(instr: &Instruction<'_>) -> Option<Vec<SymbolKey>> {
    use wast::core::Instruction;

    match instr {
        Instruction::Call(call) | Instruction::ReturnCall(call) => {
            Some(vec![SymbolKey::Function(index_as_u32(*call))])
        }
        Instruction::CallIndirect(call) | Instruction::ReturnCallIndirect(call) => {
            Some(vec![SymbolKey::Table(index_as_u32(call.table))])
        }
        Instruction::TableGet(arg)
        | Instruction::TableSet(arg)
        | Instruction::TableFill(arg)
        | Instruction::TableSize(arg)
        | Instruction::TableGrow(arg) => Some(vec![SymbolKey::Table(index_as_u32(arg.dst))]),
        Instruction::TableInit(init) => Some(vec![SymbolKey::Table(index_as_u32(init.table))]),
        Instruction::TableCopy(copy) => Some(vec![
            SymbolKey::Table(index_as_u32(copy.dst)),
            SymbolKey::Table(index_as_u32(copy.src)),
        ]),
        Instruction::TableAtomicGet(arg)
        | Instruction::TableAtomicSet(arg)
        | Instruction::TableAtomicRmwXchg(arg)
        | Instruction::TableAtomicRmwCmpxchg(arg) => {
            Some(vec![SymbolKey::Table(index_as_u32(arg.inner.dst))])
        }
        _ => None,
    }
}

fn index_as_u32(index: Index<'_>) -> u32 {
    match index {
        Index::Num(index, _) => index,
        Index::Id(_) => panic!("expected indices to be resolved to numeric indices"),
    }
}

fn build_link_info<'a>(
    wat: &'a str,
    rwat: &'a RelocWat<'a>,
    fields: &[ModuleField<'a>],
) -> Result<LinkInfo<'a>> {
    let mut imported_functions = Vec::new();
    let mut imported_tables = Vec::new();
    let mut defined_funcs = Vec::new();
    let mut defined_tables = Vec::new();
    let mut linking = HashSet::new();
    let mut import_annotations = rwat.import_annotations.iter();
    let mut func_annotations = rwat.func_annotations.iter();
    let mut table_annotations = rwat.table_annotations.iter();

    for field in fields {
        match field {
            ModuleField::Import(import) => {
                let annotation = import_annotations
                    .next()
                    .expect("expected import annotations to line up with resolved module imports");
                for (sig, sym) in import
                    .item_sigs()
                    .into_iter()
                    .zip(annotation.syms.iter().copied())
                {
                    match &sig.kind {
                        ItemKind::Func(_) | ItemKind::FuncExact(_) => {
                            if sym.is_some() {
                                linking.insert(SymbolKey::Function(
                                    u32::try_from(imported_functions.len()).unwrap(),
                                ));
                            }
                            imported_functions.push(sym);
                        }
                        ItemKind::Table(_) => {
                            if sym.is_some() {
                                linking.insert(SymbolKey::Table(
                                    u32::try_from(imported_tables.len()).unwrap(),
                                ));
                            }
                            imported_tables.push(sym);
                        }
                        _ => {}
                    }
                }
            }
            ModuleField::Func(func) => {
                let annotation = func_annotations.next().expect(
                    "expected function annotations to line up with resolved module functions",
                );
                let symbol_name = match annotation.sym {
                    Some(Some(name)) => Some(name),
                    Some(None) | None => func.id.as_ref().map(|id| id.name()),
                };
                let reloc_instrs = resolve_func_relocs(func, &annotation.reloc_spans, &mut linking);
                let index = u32::try_from(imported_functions.len() + defined_funcs.len()).unwrap();
                if annotation.sym.is_some() {
                    linking.insert(SymbolKey::Function(index));
                }
                defined_funcs.push(DefinedFunc {
                    symbol_name,
                    reloc_instrs,
                    span: func.span,
                });
            }
            ModuleField::Table(table) => {
                let annotation = table_annotations
                    .next()
                    .expect("expected table annotations to line up with resolved module tables");
                let symbol_name = match annotation.sym {
                    Some(Some(name)) => Some(name),
                    Some(None) | None => table.id.as_ref().map(|id| id.name()),
                };
                match &table.kind {
                    TableKind::Import { .. } => {
                        let index = u32::try_from(imported_tables.len()).unwrap();
                        if annotation.sym.is_some() {
                            linking.insert(SymbolKey::Table(index));
                        }
                        imported_tables.push(annotation.sym);
                    }
                    _ => {
                        let index =
                            u32::try_from(imported_tables.len() + defined_tables.len()).unwrap();
                        if annotation.sym.is_some() {
                            linking.insert(SymbolKey::Table(index));
                        }
                        defined_tables.push(DefinedTable {
                            symbol_name,
                            span: table.span,
                        });
                    }
                }
            }
            _ => continue,
        }
    }

    assert!(
        import_annotations.next().is_none(),
        "all import annotations should be consumed",
    );
    assert!(
        func_annotations.next().is_none(),
        "all function annotations should be consumed",
    );
    assert!(
        table_annotations.next().is_none(),
        "all table annotations should be consumed",
    );

    let num_imports = u32::try_from(imported_functions.len()).unwrap();
    let num_imported_tables = u32::try_from(imported_tables.len()).unwrap();

    let mut symbols = Vec::new();
    let mut symbol_indices = HashMap::new();

    for (index, sym) in imported_functions.iter().enumerate() {
        let index = u32::try_from(index).unwrap();
        if !linking.contains(&SymbolKey::Function(index)) {
            continue;
        }
        symbol_indices.insert(
            SymbolKey::Function(index),
            u32::try_from(symbols.len()).unwrap(),
        );
        symbols.push(Symbol::FunctionImport {
            index,
            explicit_name: sym.unwrap_or(None),
        });
    }

    for (offset, func) in defined_funcs.iter().enumerate() {
        let index = num_imports + u32::try_from(offset).unwrap();
        if !linking.contains(&SymbolKey::Function(index)) {
            continue;
        }
        symbol_indices.insert(
            SymbolKey::Function(index),
            u32::try_from(symbols.len()).unwrap(),
        );
        let Some(symbol_name) = func.symbol_name else {
            return Err(error(
                wat,
                func.span,
                "defined function symbols require an explicit `@sym (name ...)` or function identifier",
            ));
        };
        symbols.push(Symbol::FunctionDefined { index, symbol_name });
    }

    for (index, sym) in imported_tables.iter().enumerate() {
        let index = u32::try_from(index).unwrap();
        symbol_indices.insert(
            SymbolKey::Table(index),
            u32::try_from(symbols.len()).unwrap(),
        );
        symbols.push(Symbol::TableImport {
            index,
            explicit_name: sym.unwrap_or(None),
        });
    }

    for (offset, table) in defined_tables.iter().enumerate() {
        let index = num_imported_tables + u32::try_from(offset).unwrap();
        if !linking.contains(&SymbolKey::Table(index)) {
            continue;
        }
        symbol_indices.insert(
            SymbolKey::Table(index),
            u32::try_from(symbols.len()).unwrap(),
        );
        let Some(symbol_name) = table.symbol_name else {
            return Err(error(
                wat,
                table.span,
                "defined table symbols require an explicit `@sym (name ...)` or table identifier",
            ));
        };
        symbols.push(Symbol::TableDefined { index, symbol_name });
    }

    Ok(LinkInfo {
        symbols,
        symbol_indices,
        defined_funcs,
    })
}

fn patch_code_section(
    wasm: &[u8],
    sections: &[RawSection],
    defined_funcs: &[DefinedFunc<'_>],
) -> Result<Option<PatchedCodeSection>> {
    let Some(code_section) = sections
        .iter()
        .find(|section| section.id == CODE_SECTION_ID)
    else {
        return Ok(None);
    };

    let mut code_payload = Vec::new();
    u32::try_from(defined_funcs.len())
        .unwrap()
        .encode(&mut code_payload);

    let mut relocations = Vec::new();
    let code_reader = CodeSectionReader::new(BinaryReader::new(
        &wasm[code_section.range.clone()],
        code_section.range.start,
    ))
    .expect("expected generated code section payload to decode successfully");

    assert_eq!(
        defined_funcs.len(),
        usize::try_from(code_reader.count()).unwrap(),
        "expected code section bodies to line up with defined functions"
    );

    for (body_index, body) in code_reader.into_iter().enumerate() {
        let body = body.expect("expected generated function bodies to decode successfully");

        let func = defined_funcs
            .get(body_index)
            .expect("expected code section bodies to line up with defined functions");

        let patches = reloc_patches(&body, func);
        let mut patched_body = Vec::with_capacity(body.as_bytes().len() + patches.len() * 4);
        let mut body_relocations = Vec::with_capacity(patches.len());
        let mut cursor = 0usize;
        let mut shift = 0usize;

        for patch in patches {
            patched_body.extend_from_slice(&body.as_bytes()[cursor..patch.immediate_start]);
            let target_index = match patch.target {
                SymbolKey::Function(index) | SymbolKey::Table(index) => index,
            };
            encode_5_byte_u32_leb(target_index, &mut patched_body);
            body_relocations.push(CodeRelocation {
                // Still relative to the current function body; adjusted to code-section offset below.
                offset: u32::try_from(patch.immediate_start + shift).unwrap(),
                reloc_type: patch.reloc_type,
                target: patch.target,
            });
            // Offset within the original body bytes.
            cursor = patch.immediate_start + patch.original_len;
            // Reloc index with 5 bytes LEB128-encoded,
            // `shift` is the byte growth of `patched_body` relative to the original body.
            shift += 5 - patch.original_len;
        }
        patched_body.extend_from_slice(&body.as_bytes()[cursor..]);

        // Offset of `patched_body` within the code section.
        // size_of(code_payload) + size_of(leb(size_of(patched_body)))
        let patched_body_len = u32::try_from(patched_body.len()).unwrap();
        let body_start_in_code =
            u32::try_from(code_payload.len() + u32_leb_len(patched_body_len)).unwrap();
        for reloc in &mut body_relocations {
            // Adjust the body-relative relocation offset into a code-section-relative offset.
            reloc.offset += body_start_in_code;
        }
        relocations.extend(body_relocations);

        // Encode the current function body length before the body bytes.
        u32::try_from(patched_body.len())
            .unwrap()
            .encode(&mut code_payload);
        code_payload.extend_from_slice(&patched_body);
    }

    Ok(Some(PatchedCodeSection {
        section_index: code_section.index,
        payload: code_payload,
        relocations,
    }))
}

#[derive(Debug)]
struct CallPatch {
    immediate_start: usize,
    original_len: usize,
    reloc_type: u8,
    target: SymbolKey,
}

fn reloc_patches(body: &FunctionBody<'_>, func: &DefinedFunc<'_>) -> Vec<CallPatch> {
    let mut patches = Vec::new();
    let mut reloc_instrs = func.reloc_instrs.iter();
    let mut reader = body
        .get_operators_reader()
        .expect("expected generated function bodies to decode successfully");

    while !reader.eof() {
        let offset = reader.original_position();
        let operator = reader
            .read()
            .expect("expected generated operators to decode successfully");

        let Some(instr_patches) = operator_reloc_patches(&operator, offset, body.range().start)
        else {
            continue;
        };

        let reloc_instr = reloc_instrs.next().expect(
            "expected compiled wasm relocatable instructions to line up with the parsed AST",
        );

        if reloc_instr.has_reloc {
            patches.extend(instr_patches);
        }
    }

    assert!(
        reloc_instrs.next().is_none(),
        "expected compiled wasm relocatable instructions to line up with the parsed AST",
    );

    patches
}

fn operator_reloc_patches(
    operator: &Operator<'_>,
    offset: usize,
    body_start: usize,
) -> Option<Vec<CallPatch>> {
    fn body_relative(offset: usize, body_start: usize) -> usize {
        offset.saturating_sub(body_start)
    }

    fn prefixed_start(offset: usize, subopcode: u32, body_start: usize) -> usize {
        body_relative(offset + 1 + u32_leb_len(subopcode), body_start)
    }

    match *operator {
        Operator::Call { function_index } | Operator::ReturnCall { function_index } => {
            Some(vec![CallPatch {
                immediate_start: body_relative(offset + 1, body_start),
                original_len: u32_leb_len(function_index),
                reloc_type: RELOC_FUNCTION_INDEX_LEB,
                target: SymbolKey::Function(function_index),
            }])
        }
        Operator::CallIndirect {
            type_index,
            table_index,
        }
        | Operator::ReturnCallIndirect {
            type_index,
            table_index,
        } => Some(vec![CallPatch {
            immediate_start: body_relative(offset + 1 + u32_leb_len(type_index), body_start),
            original_len: u32_leb_len(table_index),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table_index),
        }]),
        Operator::TableGet { table } | Operator::TableSet { table } => Some(vec![CallPatch {
            immediate_start: body_relative(offset + 1, body_start),
            original_len: u32_leb_len(table),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table),
        }]),
        Operator::TableInit { elem_index, table } => Some(vec![CallPatch {
            immediate_start: prefixed_start(offset, 0x0c, body_start) + u32_leb_len(elem_index),
            original_len: u32_leb_len(table),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table),
        }]),
        Operator::TableCopy {
            dst_table,
            src_table,
        } => {
            let first_immediate_start = prefixed_start(offset, 0x0e, body_start);
            Some(vec![
                CallPatch {
                    immediate_start: first_immediate_start,
                    original_len: u32_leb_len(dst_table),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(dst_table),
                },
                CallPatch {
                    immediate_start: first_immediate_start + u32_leb_len(dst_table),
                    original_len: u32_leb_len(src_table),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(src_table),
                },
            ])
        }
        Operator::TableFill { table }
        | Operator::TableSize { table }
        | Operator::TableGrow { table } => Some(vec![CallPatch {
            immediate_start: prefixed_start(
                offset,
                match operator {
                    Operator::TableFill { .. } => 0x11,
                    Operator::TableSize { .. } => 0x10,
                    Operator::TableGrow { .. } => 0x0f,
                    _ => unreachable!(),
                },
                body_start,
            ),
            original_len: u32_leb_len(table),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table),
        }]),
        Operator::TableAtomicGet { table_index, .. }
        | Operator::TableAtomicSet { table_index, .. }
        | Operator::TableAtomicRmwXchg { table_index, .. }
        | Operator::TableAtomicRmwCmpxchg { table_index, .. } => Some(vec![CallPatch {
            immediate_start: prefixed_start(
                offset,
                match operator {
                    Operator::TableAtomicGet { .. } => 0x58,
                    Operator::TableAtomicSet { .. } => 0x59,
                    Operator::TableAtomicRmwXchg { .. } => 0x5a,
                    Operator::TableAtomicRmwCmpxchg { .. } => 0x5b,
                    _ => unreachable!(),
                },
                body_start,
            ) + 1,
            original_len: u32_leb_len(table_index),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table_index),
        }]),
        _ => None,
    }
}

fn linking_section(link_info: &LinkInfo<'_>) -> Option<LinkingSection> {
    if link_info.symbols.is_empty() {
        return None;
    }

    let mut linking = LinkingSection::new();
    let mut symbols = SymbolTable::new();
    for symbol in &link_info.symbols {
        match *symbol {
            Symbol::FunctionImport {
                index,
                explicit_name,
            } => {
                // Indicating that this symbol is not defined.
                // For non-data symbols, this must match whether the symbol is an import
                // or is defined.
                let mut flags = SymbolTable::WASM_SYM_UNDEFINED;
                if explicit_name.is_some() {
                    // The symbol uses an explicit symbol name,
                    // rather than reusing the name from a wasm import.
                    flags |= SymbolTable::WASM_SYM_EXPLICIT_NAME;
                }
                symbols.function(flags, index, explicit_name);
            }
            Symbol::FunctionDefined { index, symbol_name } => {
                symbols.function(0, index, Some(symbol_name));
            }
            Symbol::TableImport {
                index,
                explicit_name,
            } => {
                let mut flags = SymbolTable::WASM_SYM_UNDEFINED;
                if explicit_name.is_some() {
                    flags |= SymbolTable::WASM_SYM_EXPLICIT_NAME;
                }
                symbols.table(flags, index, explicit_name);
            }
            Symbol::TableDefined { index, symbol_name } => {
                symbols.table(0, index, Some(symbol_name));
            }
        }
    }
    linking.symbol_table(&symbols);
    Some(linking)
}

fn encode_reloc_code_section(
    link_info: &LinkInfo<'_>,
    code: Option<&PatchedCodeSection>,
) -> Option<Vec<u8>> {
    let code = code?;
    if code.relocations.is_empty() {
        return None;
    }

    let mut data = Vec::new();
    code.section_index.encode(&mut data);

    u32::try_from(code.relocations.len())
        .unwrap()
        .encode(&mut data);

    for reloc in &code.relocations {
        data.push(reloc.reloc_type);
        // offset of the reloc index
        reloc.offset.encode(&mut data);
        // the index of the reloc symbol
        link_info.symbol_indices[&reloc.target].encode(&mut data);
    }

    Some(data)
}

fn emit_module(
    wasm: Vec<u8>,
    sections: Vec<RawSection>,
    patched_code: Option<PatchedCodeSection>,
    linking: Option<LinkingSection>,
    reloc: Option<Vec<u8>>,
) -> Vec<u8> {
    fn emit_object_sections(
        out: &mut WasmModule,
        linking: Option<&LinkingSection>,
        reloc: Option<&[u8]>,
    ) {
        // The "linking" custom section must be after the
        // data section in order to validate data symbols.
        if let Some(linking) = linking {
            out.section(linking);
        }
        // The "reloc." custom sections must come after the
        // "linking" custom section in order to validate
        // relocation indices.
        if let Some(reloc) = reloc {
            out.section(&CustomSection {
                name: "reloc.CODE".into(),
                data: reloc.into(),
            });
        }
    }

    let mut out = WasmModule::new();

    // Object custom sections go after the last non-custom section.
    let insert_idx = sections.iter().rposition(|section| section.id != 0);
    if insert_idx.is_none() {
        emit_object_sections(&mut out, linking.as_ref(), reloc.as_deref());
    }

    for (i, section) in sections.into_iter().enumerate() {
        let section = match (section.id == CODE_SECTION_ID, patched_code.as_ref()) {
            (true, Some(patched_code)) => WasmRawSection {
                id: CODE_SECTION_ID,
                data: &patched_code.payload,
            },
            _ => WasmRawSection {
                id: section.id,
                data: &wasm[section.range],
            },
        };
        out.section(&section);

        if insert_idx == Some(i) {
            emit_object_sections(&mut out, linking.as_ref(), reloc.as_deref());
        }
    }

    out.finish()
}

fn raw_sections(wasm: &[u8]) -> Vec<RawSection> {
    let mut sections = Vec::new();

    for (index, (id, payload_range)) in
        (0u32..).zip(WasmParser::new(0).parse_all(wasm).filter_map(|payload| {
            let payload = payload.expect("expected generated wasm to parse successfully");
            payload.as_section()
        }))
    {
        sections.push(RawSection {
            id,
            index,
            range: payload_range,
        });
    }

    sections
}

fn u32_leb_len(value: u32) -> usize {
    match value {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    }
}

fn encode_5_byte_u32_leb(value: u32, dst: &mut Vec<u8>) {
    let mut value = value;
    for _ in 0..4 {
        dst.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    dst.push(value as u8);
}

fn error(wat: &str, span: Span, msg: impl Into<String>) -> wast::Error {
    let mut err = wast::Error::new(span, msg.into());
    err.set_text(wat);
    err
}
