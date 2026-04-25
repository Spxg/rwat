use wasm_encoder::{
    CustomSection, LinkingSection, Module as WasmModule, RawSection as WasmRawSection,
};
use wasmparser::Parser as WasmParser;

use crate::types::{CODE_SECTION_ID, PatchedCodeSection, RawSection};

pub(crate) fn emit_module(
    wasm: Vec<u8>,
    sections: Vec<RawSection>,
    patched_code: Option<PatchedCodeSection>,
    linking: Option<LinkingSection>,
    reloc: Option<Vec<u8>>,
) -> Vec<u8> {
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

pub(crate) fn raw_sections(wasm: &[u8]) -> Vec<RawSection> {
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
