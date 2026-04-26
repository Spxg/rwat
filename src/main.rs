//! Command-line interface for parsing annotated wat into a relocatable wasm
//! object file.
//!
//! The binary reads wat from a file or standard input, passes it to
//! `rwat::parse_rwat`, and writes the resulting wasm bytes to the requested
//! output path.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

enum Input {
    Stdin,
    File(PathBuf),
}

struct Args {
    input: Input,
    output: PathBuf,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err == usage() => {
            println!("{err}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args_os().skip(1))?;
    let wat = read_input(args.input)?;
    let wasm = rwat::parse_rwat(&wat).map_err(|err| err.to_string())?;
    fs::write(&args.output, wasm)
        .map_err(|err| format!("failed to write `{}`: {err}", args.output.display()))?;
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut input = None;
    let mut output = None;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => return Err(usage()),
            "-o" | "--output" => {
                let Some(path) = iter.next() else {
                    return Err(format!(
                        "missing value for `{}`\n\n{}",
                        arg.to_string_lossy(),
                        usage()
                    ));
                };
                output = Some(PathBuf::from(path));
            }
            _ if arg.to_string_lossy().starts_with('-') && arg != "-" => {
                return Err(format!(
                    "unknown option `{}`\n\n{}",
                    arg.to_string_lossy(),
                    usage()
                ));
            }
            _ => {
                if input.is_some() {
                    return Err(format!(
                        "unexpected extra input `{}`\n\n{}",
                        arg.to_string_lossy(),
                        usage()
                    ));
                }
                input = Some(if arg == "-" {
                    Input::Stdin
                } else {
                    Input::File(PathBuf::from(arg))
                });
            }
        }
    }

    let Some(output) = output else {
        return Err(format!("missing output file\n\n{}", usage()));
    };

    Ok(Args {
        input: input.unwrap_or(Input::Stdin),
        output,
    })
}

fn read_input(input: Input) -> Result<String, String> {
    match input {
        Input::Stdin => {
            let mut wat = String::new();
            io::stdin()
                .read_to_string(&mut wat)
                .map_err(|err| format!("failed to read stdin: {err}"))?;
            Ok(wat)
        }
        Input::File(path) => fs::read_to_string(&path)
            .map_err(|err| format!("failed to read `{}`: {err}", path.display())),
    }
}

fn usage() -> String {
    "usage: rwat [wat|-] -o|--output <wasm>\n\nReads wat from a file, or stdin when no input file or `-` is given.".to_owned()
}
