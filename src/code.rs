use wasm_encoder::Encode;
use wasmparser::{BinaryReader, CodeSectionReader, FunctionBody};
use wast::parser::Result;

use crate::reloc::{operator_patches, u32_leb_len};
use crate::types::{
    CODE_SECTION_ID, CodeRelocation, DefinedFunc, LinkInfo, PatchedCodeSection, RawSection,
};

pub(crate) fn encode_reloc_code_section(
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
        // Offset of the reloc index.
        reloc.offset.encode(&mut data);
        // The index of the reloc symbol.
        link_info.symbol_indices[&reloc.target].encode(&mut data);
    }

    Some(data)
}

pub(crate) fn patch_code_section(
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

        let (patched_body, mut body_relocations) = patch_function_body(&body, func);

        // Offset of `patched_body` within the code section:
        // size_of(code_payload) + size_of(leb(size_of(patched_body))).
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

fn patch_function_body(
    body: &FunctionBody<'_>,
    func: &DefinedFunc<'_>,
) -> (Vec<u8>, Vec<CodeRelocation>) {
    let patches = reloc_patches(body, func);
    let mut patched_body = Vec::with_capacity(body.as_bytes().len() + patches.len() * 4);
    let mut relocations = Vec::with_capacity(patches.len());
    let mut cursor = 0usize;
    let mut shift = 0usize;

    for patch in patches {
        patched_body.extend_from_slice(&body.as_bytes()[cursor..patch.immediate_start]);
        encode_5_byte_u32_leb(patch.target.index(), &mut patched_body);
        relocations.push(CodeRelocation {
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

    (patched_body, relocations)
}

fn reloc_patches(body: &FunctionBody<'_>, func: &DefinedFunc<'_>) -> Vec<crate::reloc::RelocPatch> {
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

        let Some(instr_patches) = operator_patches(&operator, offset, body.range().start) else {
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

fn encode_5_byte_u32_leb(value: u32, dst: &mut Vec<u8>) {
    let mut value = value;
    for _ in 0..4 {
        dst.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    dst.push(value as u8);
}
