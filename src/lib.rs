mod code;
mod emit;
mod link;
mod parse;
mod reloc;
mod scan;
mod types;

mod annotation {
    wast::annotation!(rwat);
}

pub use parse::parse_rwat;
