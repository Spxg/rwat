use std::collections::HashMap;
use std::ops::Range;

use wast::token::Span;

/// code section's id always 10
pub(crate) const CODE_SECTION_ID: u8 = 10;

#[derive(Debug, Default)]
pub(crate) struct RelocWat<'a> {
    pub(crate) import_annotations: Vec<RelocImports<'a>>,
    pub(crate) func_annotations: Vec<FuncAnnotation<'a>>,
    pub(crate) table_annotations: Vec<TableAnnotation<'a>>,
}

#[derive(Debug)]
pub(crate) struct RelocImports<'a> {
    pub(crate) syms: Vec<SymbolAnnotation<'a>>,
}

#[derive(Debug)]
pub(crate) struct FuncAnnotation<'a> {
    pub(crate) sym: SymbolAnnotation<'a>,
    pub(crate) reloc_spans: Vec<Span>,
}

#[derive(Debug)]
pub(crate) struct TableAnnotation<'a> {
    pub(crate) sym: SymbolAnnotation<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SymbolAnnotation<'a> {
    Missing,
    Inferred,
    Explicit(&'a str),
}

impl<'a> SymbolAnnotation<'a> {
    pub(crate) fn is_present(self) -> bool {
        !matches!(self, Self::Missing)
    }

    pub(crate) fn explicit_name(self) -> Option<&'a str> {
        match self {
            Self::Explicit(name) => Some(name),
            Self::Missing | Self::Inferred => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ParsedRelocFunc<'a> {
    Import(SymbolAnnotation<'a>),
    Defined(FuncAnnotation<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SymbolKey {
    Function(u32),
    Table(u32),
}

impl SymbolKey {
    pub(crate) fn index(self) -> u32 {
        match self {
            Self::Function(index) | Self::Table(index) => index,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Symbol<'a> {
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
pub(crate) struct DefinedFunc<'a> {
    pub(crate) symbol_name: Option<&'a str>,
    pub(crate) reloc_instrs: Vec<ParsedRelocInstruction>,
    pub(crate) span: Span,
}

#[derive(Debug)]
pub(crate) struct DefinedTable<'a> {
    pub(crate) symbol_name: Option<&'a str>,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRelocInstruction {
    pub(crate) has_reloc: bool,
    pub(crate) targets: Vec<SymbolKey>,
}

#[derive(Debug)]
pub(crate) struct LinkInfo<'a> {
    pub(crate) symbols: Vec<Symbol<'a>>,
    pub(crate) symbol_indices: HashMap<SymbolKey, u32>,
    pub(crate) defined_funcs: Vec<DefinedFunc<'a>>,
}

#[derive(Debug)]
pub(crate) struct CodeRelocation {
    pub(crate) offset: u32,
    pub(crate) reloc_type: u8,
    pub(crate) target: SymbolKey,
}

#[derive(Debug)]
pub(crate) struct PatchedCodeSection {
    pub(crate) section_index: u32,
    pub(crate) payload: Vec<u8>,
    pub(crate) relocations: Vec<CodeRelocation>,
}

#[derive(Debug, Clone)]
pub(crate) struct RawSection {
    pub(crate) id: u8,
    pub(crate) index: u32,
    pub(crate) range: Range<usize>,
}
