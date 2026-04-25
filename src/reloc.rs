use wasmparser::Operator;
use wast::core::Instruction;
use wast::token::Index;

use crate::types::SymbolKey;

const RELOC_FUNCTION_INDEX_LEB: u8 = 0;
const RELOC_TABLE_NUMBER_LEB: u8 = 20;

const TABLE_INIT_SUBOPCODE: u32 = 0x0c;
const TABLE_COPY_SUBOPCODE: u32 = 0x0e;
const TABLE_GROW_SUBOPCODE: u32 = 0x0f;
const TABLE_SIZE_SUBOPCODE: u32 = 0x10;
const TABLE_FILL_SUBOPCODE: u32 = 0x11;
const TABLE_ATOMIC_GET_SUBOPCODE: u32 = 0x58;
const TABLE_ATOMIC_SET_SUBOPCODE: u32 = 0x59;
const TABLE_ATOMIC_RMW_XCHG_SUBOPCODE: u32 = 0x5a;
const TABLE_ATOMIC_RMW_CMPXCHG_SUBOPCODE: u32 = 0x5b;

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
) -> Option<Vec<RelocPatch>> {
    match *operator {
        Operator::Call { function_index } | Operator::ReturnCall { function_index } => {
            Some(vec![RelocPatch {
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
        } => Some(vec![RelocPatch {
            immediate_start: body_relative(offset + 1 + u32_leb_len(type_index), body_start),
            original_len: u32_leb_len(table_index),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table_index),
        }]),
        Operator::TableGet { table } | Operator::TableSet { table } => Some(vec![RelocPatch {
            immediate_start: body_relative(offset + 1, body_start),
            original_len: u32_leb_len(table),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table),
        }]),
        Operator::TableInit { elem_index, table } => Some(vec![RelocPatch {
            immediate_start: prefixed_start(offset, TABLE_INIT_SUBOPCODE, body_start)
                + u32_leb_len(elem_index),
            original_len: u32_leb_len(table),
            reloc_type: RELOC_TABLE_NUMBER_LEB,
            target: SymbolKey::Table(table),
        }]),
        Operator::TableCopy {
            dst_table,
            src_table,
        } => {
            let first_immediate_start = prefixed_start(offset, TABLE_COPY_SUBOPCODE, body_start);
            Some(vec![
                RelocPatch {
                    immediate_start: first_immediate_start,
                    original_len: u32_leb_len(dst_table),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(dst_table),
                },
                RelocPatch {
                    immediate_start: first_immediate_start + u32_leb_len(dst_table),
                    original_len: u32_leb_len(src_table),
                    reloc_type: RELOC_TABLE_NUMBER_LEB,
                    target: SymbolKey::Table(src_table),
                },
            ])
        }
        Operator::TableFill { table } => prefixed_table_patch(
            offset,
            body_start,
            TABLE_FILL_SUBOPCODE,
            SymbolKey::Table(table),
        ),
        Operator::TableSize { table } => prefixed_table_patch(
            offset,
            body_start,
            TABLE_SIZE_SUBOPCODE,
            SymbolKey::Table(table),
        ),
        Operator::TableGrow { table } => prefixed_table_patch(
            offset,
            body_start,
            TABLE_GROW_SUBOPCODE,
            SymbolKey::Table(table),
        ),
        Operator::TableAtomicGet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, TABLE_ATOMIC_GET_SUBOPCODE, table_index)
        }
        Operator::TableAtomicSet { table_index, .. } => {
            prefixed_table_atomic_patch(offset, body_start, TABLE_ATOMIC_SET_SUBOPCODE, table_index)
        }
        Operator::TableAtomicRmwXchg { table_index, .. } => prefixed_table_atomic_patch(
            offset,
            body_start,
            TABLE_ATOMIC_RMW_XCHG_SUBOPCODE,
            table_index,
        ),
        Operator::TableAtomicRmwCmpxchg { table_index, .. } => prefixed_table_atomic_patch(
            offset,
            body_start,
            TABLE_ATOMIC_RMW_CMPXCHG_SUBOPCODE,
            table_index,
        ),
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
    subopcode: u32,
    target: SymbolKey,
) -> Option<Vec<RelocPatch>> {
    Some(vec![RelocPatch {
        immediate_start: prefixed_start(offset, subopcode, body_start),
        original_len: u32_leb_len(target.index()),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target,
    }])
}

fn prefixed_table_atomic_patch(
    offset: usize,
    body_start: usize,
    subopcode: u32,
    table_index: u32,
) -> Option<Vec<RelocPatch>> {
    Some(vec![RelocPatch {
        immediate_start: prefixed_start(offset, subopcode, body_start) + 1,
        original_len: u32_leb_len(table_index),
        reloc_type: RELOC_TABLE_NUMBER_LEB,
        target: SymbolKey::Table(table_index),
    }])
}

fn body_relative(offset: usize, body_start: usize) -> usize {
    offset.saturating_sub(body_start)
}

fn prefixed_start(offset: usize, subopcode: u32, body_start: usize) -> usize {
    body_relative(offset + 1 + u32_leb_len(subopcode), body_start)
}

fn index_as_u32(index: Index<'_>) -> u32 {
    match index {
        Index::Num(index, _) => index,
        Index::Id(_) => panic!("expected indices to be resolved to numeric indices"),
    }
}
