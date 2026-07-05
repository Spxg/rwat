use wasmparser::Operator;
use wast::core::Instruction;
use wast::token::Index;

use crate::types::SymbolKey;

const RELOC_FUNCTION_INDEX_LEB: u8 = 0;
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
    body_start: usize,
    body: &[u8],
) -> Option<Vec<RelocPatch>> {
    match *operator {
        Operator::Call { function_index } | Operator::ReturnCall { function_index } => {
            let immediate_start = body_relative(offset + 1, body_start);
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_FUNCTION_INDEX_LEB,
                target: SymbolKey::Function(function_index),
            }])
        }
        Operator::CallIndirect { table_index, .. }
        | Operator::ReturnCallIndirect { table_index, .. } => {
            let type_start = body_relative(offset + 1, body_start);
            let immediate_start = type_start + u32_leb_len_at(body, type_start);
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TABLE_NUMBER_LEB,
                target: SymbolKey::Table(table_index),
            }])
        }
        Operator::TableGet { table } | Operator::TableSet { table } => {
            let immediate_start = body_relative(offset + 1, body_start);
            Some(vec![RelocPatch {
                immediate_start,
                original_len: u32_leb_len_at(body, immediate_start),
                reloc_type: RELOC_TABLE_NUMBER_LEB,
                target: SymbolKey::Table(table),
            }])
        }
        Operator::TableInit { table, .. } => {
            let elem_start = prefixed_start(offset, body_start, body);
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
            let first_immediate_start = prefixed_start(offset, body_start, body);
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
            prefixed_table_patch(offset, body_start, body, SymbolKey::Table(table))
        }
        Operator::TableSize { table } => {
            prefixed_table_patch(offset, body_start, body, SymbolKey::Table(table))
        }
        Operator::TableGrow { table } => {
            prefixed_table_patch(offset, body_start, body, SymbolKey::Table(table))
        }
        Operator::TableAtomicGet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, body, table_index)
        }
        Operator::TableAtomicSet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, body, table_index)
        }
        Operator::TableAtomicRmwXchg { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, body, table_index)
        }
        Operator::TableAtomicRmwCmpxchg { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, body, table_index)
        }
        _ => None,
    }
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

fn prefixed_table_patch(
    offset: usize,
    body_start: usize,
    body: &[u8],
    target: SymbolKey,
) -> Option<Vec<RelocPatch>> {
    let immediate_start = prefixed_start(offset, body_start, body);
    Some(vec![RelocPatch {
        immediate_start,
        original_len: u32_leb_len_at(body, immediate_start),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target,
    }])
}

fn prefixed_table_atomic_patch(
    offset: usize,
    body_start: usize,
    body: &[u8],
    table_index: u32,
) -> Option<Vec<RelocPatch>> {
    let immediate_start = prefixed_start(offset, body_start, body) + 1;
    Some(vec![RelocPatch {
        immediate_start,
        original_len: u32_leb_len_at(body, immediate_start),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target: SymbolKey::Table(table_index),
    }])
}

fn body_relative(offset: usize, body_start: usize) -> usize {
    offset.saturating_sub(body_start)
}

fn prefixed_start(offset: usize, body_start: usize, body: &[u8]) -> usize {
    let subopcode_start = body_relative(offset + 1, body_start);
    subopcode_start + u32_leb_len_at(body, subopcode_start)
}

fn u32_leb_len_at(body: &[u8], offset: usize) -> usize {
    for (i, byte) in body[offset..].iter().take(5).enumerate() {
        if byte & 0x80 == 0 {
            return i + 1;
        }
    }
    panic!("expected valid u32 LEB immediate in generated function body")
}

fn index_as_u32(index: Index<'_>) -> u32 {
    match index {
        Index::Num(index, _) => index,
        Index::Id(_) => panic!("expected indices to be resolved to numeric indices"),
    }
}
