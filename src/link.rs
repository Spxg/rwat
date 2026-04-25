use std::collections::{HashMap, HashSet};

use wasm_encoder::{LinkingSection, SymbolTable};
use wast::core::{Func, FuncKind, Imports, ItemKind, ModuleField, Table, TableKind};
use wast::parser::Result;
use wast::token::Id;

use crate::parse::error;
use crate::reloc;
use crate::types::{
    DefinedFunc, DefinedTable, FuncAnnotation, LinkInfo, ParsedRelocInstruction, RelocImports,
    RelocWat, Symbol, SymbolAnnotation, SymbolKey, TableAnnotation,
};

pub(crate) fn build_link_info<'a>(
    wat: &'a str,
    rwat: &'a RelocWat<'a>,
    fields: &[ModuleField<'a>],
) -> Result<LinkInfo<'a>> {
    let mut builder = LinkInfoBuilder::default();
    let mut import_annotations = rwat.import_annotations.iter();
    let mut func_annotations = rwat.func_annotations.iter();
    let mut table_annotations = rwat.table_annotations.iter();

    for field in fields {
        match field {
            ModuleField::Import(import) => {
                let annotation = import_annotations
                    .next()
                    .expect("expected import annotations to line up with resolved module imports");
                builder.visit_import(import, annotation);
            }
            ModuleField::Func(func) => {
                let annotation = func_annotations.next().expect(
                    "expected function annotations to line up with resolved module functions",
                );
                builder.visit_func(func, annotation);
            }
            ModuleField::Table(table) => {
                let annotation = table_annotations
                    .next()
                    .expect("expected table annotations to line up with resolved module tables");
                builder.visit_table(table, annotation);
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

    builder.finish(wat)
}

pub(crate) fn linking_section(link_info: &LinkInfo<'_>) -> Option<LinkingSection> {
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
                // Indicating that this symbol is not defined.
                // For non-data symbols, this must match whether the symbol is an import
                // or is defined.
                let mut flags = SymbolTable::WASM_SYM_UNDEFINED;
                if explicit_name.is_some() {
                    // The symbol uses an explicit symbol name,
                    // rather than reusing the name from a wasm import.
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

#[derive(Debug, Default)]
struct LinkInfoBuilder<'a> {
    imported_functions: Vec<SymbolAnnotation<'a>>,
    imported_tables: Vec<SymbolAnnotation<'a>>,
    defined_funcs: Vec<DefinedFunc<'a>>,
    defined_tables: Vec<DefinedTable<'a>>,
    linking: HashSet<SymbolKey>,
}

impl<'a> LinkInfoBuilder<'a> {
    fn visit_import(&mut self, import: &Imports<'a>, annotation: &RelocImports<'a>) {
        for (sig, sym) in import
            .item_sigs()
            .into_iter()
            .zip(annotation.syms.iter().copied())
        {
            match &sig.kind {
                ItemKind::Func(_) | ItemKind::FuncExact(_) => self.push_imported_function(sym),
                ItemKind::Table(_) => self.push_imported_table(sym),
                _ => {}
            }
        }
    }

    fn visit_func(&mut self, func: &Func<'a>, annotation: &FuncAnnotation<'a>) {
        let index = self.next_defined_function_index();
        let symbol_name = inferred_symbol_name(annotation.sym, func.id.as_ref());
        let reloc_instrs = resolve_func_relocs(func, &annotation.reloc_spans, &mut self.linking);

        self.mark_function_if_present(index, annotation.sym);
        self.defined_funcs.push(DefinedFunc {
            symbol_name,
            reloc_instrs,
            span: func.span,
        });
    }

    fn visit_table(&mut self, table: &Table<'a>, annotation: &TableAnnotation<'a>) {
        match &table.kind {
            TableKind::Import { .. } => self.push_imported_table(annotation.sym),
            _ => {
                let index = self.next_defined_table_index();
                self.mark_table_if_present(index, annotation.sym);
                self.defined_tables.push(DefinedTable {
                    symbol_name: inferred_symbol_name(annotation.sym, table.id.as_ref()),
                    span: table.span,
                });
            }
        }
    }

    fn push_imported_function(&mut self, sym: SymbolAnnotation<'a>) {
        let index = u32::try_from(self.imported_functions.len()).unwrap();
        self.mark_function_if_present(index, sym);
        self.imported_functions.push(sym);
    }

    fn push_imported_table(&mut self, sym: SymbolAnnotation<'a>) {
        let index = u32::try_from(self.imported_tables.len()).unwrap();
        self.mark_table_if_present(index, sym);
        self.imported_tables.push(sym);
    }

    fn next_defined_function_index(&self) -> u32 {
        u32::try_from(self.imported_functions.len() + self.defined_funcs.len()).unwrap()
    }

    fn next_defined_table_index(&self) -> u32 {
        u32::try_from(self.imported_tables.len() + self.defined_tables.len()).unwrap()
    }

    fn mark_function_if_present(&mut self, index: u32, sym: SymbolAnnotation<'a>) {
        if sym.is_present() {
            self.linking.insert(SymbolKey::Function(index));
        }
    }

    fn mark_table_if_present(&mut self, index: u32, sym: SymbolAnnotation<'a>) {
        if sym.is_present() {
            self.linking.insert(SymbolKey::Table(index));
        }
    }

    fn finish(self, wat: &'a str) -> Result<LinkInfo<'a>> {
        let num_imported_functions = u32::try_from(self.imported_functions.len()).unwrap();
        let num_imported_tables = u32::try_from(self.imported_tables.len()).unwrap();

        let mut symbols = Vec::new();
        let mut symbol_indices = HashMap::new();

        self.add_imported_function_symbols(&mut symbols, &mut symbol_indices);
        self.add_defined_function_symbols(
            wat,
            num_imported_functions,
            &mut symbols,
            &mut symbol_indices,
        )?;
        self.add_imported_table_symbols(&mut symbols, &mut symbol_indices);
        self.add_defined_table_symbols(
            wat,
            num_imported_tables,
            &mut symbols,
            &mut symbol_indices,
        )?;

        Ok(LinkInfo {
            symbols,
            symbol_indices,
            defined_funcs: self.defined_funcs,
        })
    }

    fn add_imported_function_symbols(
        &self,
        symbols: &mut Vec<Symbol<'a>>,
        symbol_indices: &mut HashMap<SymbolKey, u32>,
    ) {
        for (index, sym) in self.imported_functions.iter().enumerate() {
            let index = u32::try_from(index).unwrap();
            let key = SymbolKey::Function(index);
            if !self.linking.contains(&key) {
                continue;
            }
            insert_symbol_index(symbol_indices, symbols.as_slice(), key);
            symbols.push(Symbol::FunctionImport {
                index,
                explicit_name: sym.explicit_name(),
            });
        }
    }

    fn add_defined_function_symbols(
        &self,
        wat: &'a str,
        num_imported_functions: u32,
        symbols: &mut Vec<Symbol<'a>>,
        symbol_indices: &mut HashMap<SymbolKey, u32>,
    ) -> Result<()> {
        for (offset, func) in self.defined_funcs.iter().enumerate() {
            let index = num_imported_functions + u32::try_from(offset).unwrap();
            let key = SymbolKey::Function(index);
            if !self.linking.contains(&key) {
                continue;
            }
            insert_symbol_index(symbol_indices, symbols.as_slice(), key);
            let Some(symbol_name) = func.symbol_name else {
                return Err(error(
                    wat,
                    func.span,
                    "defined function symbols require an explicit `@sym (name ...)` or function identifier",
                ));
            };
            symbols.push(Symbol::FunctionDefined { index, symbol_name });
        }
        Ok(())
    }

    fn add_imported_table_symbols(
        &self,
        symbols: &mut Vec<Symbol<'a>>,
        symbol_indices: &mut HashMap<SymbolKey, u32>,
    ) {
        for (index, sym) in self.imported_tables.iter().enumerate() {
            let index = u32::try_from(index).unwrap();
            let key = SymbolKey::Table(index);
            if !self.linking.contains(&key) {
                continue;
            }
            insert_symbol_index(symbol_indices, symbols.as_slice(), key);
            symbols.push(Symbol::TableImport {
                index,
                explicit_name: sym.explicit_name(),
            });
        }
    }

    fn add_defined_table_symbols(
        &self,
        wat: &'a str,
        num_imported_tables: u32,
        symbols: &mut Vec<Symbol<'a>>,
        symbol_indices: &mut HashMap<SymbolKey, u32>,
    ) -> Result<()> {
        for (offset, table) in self.defined_tables.iter().enumerate() {
            let index = num_imported_tables + u32::try_from(offset).unwrap();
            let key = SymbolKey::Table(index);
            if !self.linking.contains(&key) {
                continue;
            }
            insert_symbol_index(symbol_indices, symbols.as_slice(), key);
            let Some(symbol_name) = table.symbol_name else {
                return Err(error(
                    wat,
                    table.span,
                    "defined table symbols require an explicit `@sym (name ...)` or table identifier",
                ));
            };
            symbols.push(Symbol::TableDefined { index, symbol_name });
        }
        Ok(())
    }
}

fn resolve_func_relocs(
    func: &Func<'_>,
    reloc_spans: &[wast::token::Span],
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
        let Some(targets) = reloc::instruction_targets(instr) else {
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

fn inferred_symbol_name<'a>(sym: SymbolAnnotation<'a>, id: Option<&Id<'a>>) -> Option<&'a str> {
    match sym {
        SymbolAnnotation::Explicit(name) => Some(name),
        SymbolAnnotation::Missing | SymbolAnnotation::Inferred => id.map(Id::name),
    }
}

fn insert_symbol_index(
    symbol_indices: &mut HashMap<SymbolKey, u32>,
    symbols: &[Symbol<'_>],
    key: SymbolKey,
) {
    symbol_indices.insert(key, u32::try_from(symbols.len()).unwrap());
}
