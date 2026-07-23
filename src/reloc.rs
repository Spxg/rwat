use wasmparser::{Catch, Operator};
use wast::core::{Instruction, TryTableCatchKind};
use wast::token::Index;

use crate::types::SymbolKey;

const RELOC_FUNCTION_INDEX_LEB: u8 = 0;
const RELOC_TAG_INDEX_LEB: u8 = 10;
const RELOC_TABLE_NUMBER_LEB: u8 = 20;

#[derive(Debug)]
pub(crate) struct RelocPatch {
    pub(crate) immediate_start: usize,
    pub(crate) original_len: usize,
    pub(crate) reloc_type: u8,
    pub(crate) target: SymbolKey,
}

pub(crate) fn is_relocatable_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "call"
            | "return_call"
            | "throw"
            | "try_table"
            | "call_indirect"
            | "return_call_indirect"
            | "table.get"
            | "table.set"
            | "table.init"
            | "table.copy"
            | "table.fill"
            | "table.size"
            | "table.grow"
            | "table.atomic.get"
            | "table.atomic.set"
            | "table.atomic.rmw.xchg"
            | "table.atomic.rmw.cmpxchg"
    )
}

pub(crate) fn instruction_targets(instr: &Instruction<'_>) -> Option<Vec<SymbolKey>> {
    match instr {
        Instruction::Call(call) | Instruction::ReturnCall(call) => {
            Some(vec![SymbolKey::Function(index_as_u32(*call))])
        }
        Instruction::Throw(tag) => Some(vec![SymbolKey::Tag(index_as_u32(*tag))]),
        Instruction::TryTable(try_table) => Some(
            try_table
                .catches
                .iter()
                .filter_map(|catch| match catch.kind {
                    TryTableCatchKind::Catch(tag) | TryTableCatchKind::CatchRef(tag) => {
                        Some(SymbolKey::Tag(index_as_u32(tag)))
                    }
                    TryTableCatchKind::CatchAll | TryTableCatchKind::CatchAllRef => None,
                })
                .collect(),
        ),
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

pub(crate) fn operator_patches(
    operator: &Operator<'_>,
    offset: usize,
    body: &[u8],
) -> Option<Vec<RelocPatch>> {
    if let Operator::TryTable { try_table } = operator {
        return try_table_patches(offset, body, try_table);
    }

    match *operator {
        Operator::Call { function_index } | Operator::ReturnCall { function_index } => {
            let immediate_start = offset + 1;
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_FUNCTION_INDEX_LEB,
                target: SymbolKey::Function(function_index),
            }])
        }
        Operator::Throw { tag_index } => {
            let immediate_start = offset + 1;
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TAG_INDEX_LEB,
                target: SymbolKey::Tag(tag_index),
            }])
        }
        Operator::CallIndirect { table_index, .. }
        | Operator::ReturnCallIndirect { table_index, .. } => {
            let type_start = offset + 1;
            let immediate_start = type_start + u32_leb_len_at(body, type_start);
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TABLE_NUMBER_LEB,
                target: SymbolKey::Table(table_index),
            }])
        }
        Operator::TableGet { table } | Operator::TableSet { table } => {
            let immediate_start = offset + 1;
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TABLE_NUMBER_LEB,
                target: SymbolKey::Table(table),
            }])
        }
        Operator::TableInit { table, .. } => {
            let elem_start = prefixed_start(offset, body);
            let immediate_start = elem_start + u32_leb_len_at(body, elem_start);
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TABLE_NUMBER_LEB,
                target: SymbolKey::Table(table),
            }])
        }
        Operator::TableCopy {
            dst_table,
            src_table,
        } => {
            let first_immediate_start = prefixed_start(offset, body);
            let second_immediate_start =
                first_immediate_start + u32_leb_len_at(body, first_immediate_start);
            Some(vec![
                RelocPatch {
                    immediate_start: first_immediate_start,
                    original_len: u32_leb_len_at(body, first_immediate_start),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(dst_table),
                },
                RelocPatch {
                    immediate_start: second_immediate_start,
                    original_len: u32_leb_len_at(body, second_immediate_start),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(src_table),
                },
            ])
        }
        Operator::TableFill { table } => {
            prefixed_table_patch(offset, body, SymbolKey::Table(table))
        }
        Operator::TableSize { table } => {
            prefixed_table_patch(offset, body, SymbolKey::Table(table))
        }
        Operator::TableGrow { table } => {
            prefixed_table_patch(offset, body, SymbolKey::Table(table))
        }
        Operator::TableAtomicGet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body, table_index)
        }
        Operator::TableAtomicSet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body, table_index)
        }
        Operator::TableAtomicRmwXchg { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body, table_index)
        }
        Operator::TableAtomicRmwCmpxchg { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body, table_index)
        }
        _ => None,
    }
}

fn try_table_patches(
    offset: usize,
    body: &[u8],
    try_table: &wasmparser::TryTable,
) -> Option<Vec<RelocPatch>> {
    let mut cursor = offset + 1;
    cursor += block_type_len_at(body, cursor);
    cursor += u32_leb_len_at(body, cursor);

    let mut patches = Vec::new();
    for catch in &try_table.catches {
        cursor += 1;

        match *catch {
            Catch::One { tag, .. } | Catch::OneRef { tag, .. } => {
                let original_len = u32_leb_len_at(body, cursor);
                patches.push(RelocPatch {
                    immediate_start: cursor,
                    original_len,
                    reloc_type: RELOC_TAG_INDEX_LEB,
                    target: SymbolKey::Tag(tag),
                });
                cursor += original_len;
            }
            Catch::All { .. } | Catch::AllRef { .. } => {}
        }

        cursor += u32_leb_len_at(body, cursor);
    }

    Some(patches)
}

pub(crate) fn u32_leb_len(value: u32) -> usize {
    match value {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0x0fff_ffff => 4,
        _ => 5,
    }
}

fn prefixed_table_patch(offset: usize, body: &[u8], target: SymbolKey) -> Option<Vec<RelocPatch>> {
    let immediate_start = prefixed_start(offset, body);
    Some(vec![RelocPatch {
        immediate_start,
        original_len: u32_leb_len_at(body, immediate_start),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target,
    }])
}

fn prefixed_table_atomic_patch(
    offset: usize,
    body: &[u8],
    table_index: u32,
) -> Option<Vec<RelocPatch>> {
    let immediate_start = prefixed_start(offset, body) + 1;
    Some(vec![RelocPatch {
        immediate_start,
        original_len: u32_leb_len_at(body, immediate_start),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target: SymbolKey::Table(table_index),
    }])
}

fn prefixed_start(offset: usize, body: &[u8]) -> usize {
    let subopcode_start = offset + 1;
    subopcode_start + u32_leb_len_at(body, subopcode_start)
}

fn u32_leb_len_at(body: &[u8], offset: usize) -> usize {
    leb_len_at(body, offset, 5)
}

fn block_type_len_at(body: &[u8], offset: usize) -> usize {
    let prefix_len = usize::from(matches!(body[offset], 0x63 | 0x64));
    prefix_len + leb_len_at(body, offset + prefix_len, 5)
}

fn leb_len_at(body: &[u8], offset: usize, max_len: usize) -> usize {
    for (i, byte) in body[offset..].iter().take(max_len).enumerate() {
        if byte & 0x80 == 0 {
            return i + 1;
        }
    }
    panic!("expected valid LEB immediate in generated function body")
}

fn index_as_u32(index: Index<'_>) -> u32 {
    match index {
        Index::Num(index, _) => index,
        Index::Id(_) => panic!("expected indices to be resolved to numeric indices"),
    }
}
