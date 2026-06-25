//! Command-line interface for parsing annotated wat and linking it with lld.
//!
//! The binary reads wat from files or standard input, passes it to
//! `rwat::parse_rwat`, and invokes `lld` to link the temporary objects
//! unless `-c` requests a relocatable wasm object.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

enum Input {
    Stdin,
    File(PathBuf),
}

struct Args {
    inputs: Vec<Input>,
    output: Option<PathBuf>,
    compile_only: bool,
    linker_args: Vec<OsString>,
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
    let mut args = parse_args(env::args_os().skip(1))?;
    if args.compile_only {
        let input = args
            .inputs
            .pop()
            .expect("parse_args should provide exactly one input");
        let output = args.output.unwrap_or_else(|| match &input {
            Input::Stdin => PathBuf::from("a.o"),
            Input::File(path) => path.with_extension("o"),
        });
        let wat = read_input(input)?;
        let wasm = rwat::parse_rwat(&wat).map_err(|err| err.to_string())?;
        fs::write(&output, wasm)
            .map_err(|err| format!("failed to write `{}`: {err}", output.display()))?;
        return Ok(());
    }

    if args
        .inputs
        .iter()
        .filter(|input| matches!(input, Input::Stdin))
        .count()
        > 1
    {
        return Err("linking more than one stdin input is not supported".to_owned());
    }

    let mut temp_objects = TempObjects::default();
    let objects = args
        .inputs
        .into_iter()
        .enumerate()
        .map(|(index, input)| {
            let object = PathBuf::from(format!(".rwat-{}-{index}.o", std::process::id()));
            let wat = read_input(input)?;
            let wasm = rwat::parse_rwat(&wat).map_err(|err| err.to_string())?;
            fs::write(&object, wasm)
                .map_err(|err| format!("failed to write `{}`: {err}", object.display()))?;
            temp_objects.paths.push(object.clone());
            Ok(object)
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut command = Command::new("lld");
    command
        .arg("-flavor")
        .arg("wasm")
        .args(args.linker_args)
        .args(&objects);
    command
        .arg("-o")
        .arg(args.output.unwrap_or_else(|| PathBuf::from("a.out")));

    let output = command
        .output()
        .map_err(|err| format!("failed to run `lld -flavor wasm`: {err}"))?;

    if !output.status.success() {
        let mut message = format!("`lld -flavor wasm` failed with {}", output.status);
        if !output.stdout.is_empty() {
            message.push_str("\n\nstdout:\n");
            message.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            message.push_str("\n\nstderr:\n");
            message.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        return Err(message);
    }

    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut compile_only = false;
    let mut linker_args = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        let arg_lossy = arg.to_string_lossy();
        match arg_lossy.as_ref() {
            "-h" | "--help" => return Err(usage()),
            "-c" => compile_only = true,
            "-o" | "--output" => {
                let Some(path) = iter.next() else {
                    return Err(format!("missing value for `{}`\n\n{}", arg_lossy, usage()));
                };
                output = Some(PathBuf::from(path));
            }
            "--" => {
                linker_args.extend(iter);
                break;
            }
            _ if let Some(rest) = arg_lossy.strip_prefix("-Wl,") => {
                linker_args.extend(
                    rest.split(',')
                        .filter(|arg| !arg.is_empty())
                        .map(OsString::from),
                );
            }
            _ if arg_lossy.starts_with('-') && arg != "-" => {
                return Err(format!("unknown option `{}`\n\n{}", arg_lossy, usage()));
            }
            _ => {
                inputs.push(if arg == "-" {
                    Input::Stdin
                } else {
                    Input::File(PathBuf::from(arg))
                });
            }
        }
    }

    if inputs.is_empty() {
        inputs.push(Input::Stdin);
    }

    if compile_only && inputs.len() != 1 {
        return Err(format!("`-c` requires exactly one input\n\n{}", usage()));
    }

    Ok(Args {
        inputs,
        output,
        compile_only,
        linker_args,
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

#[derive(Default)]
struct TempObjects {
    paths: Vec<PathBuf>,
}

impl Drop for TempObjects {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

fn usage() -> String {
    "usage: rwat [-c] [wat|-]... [-o|--output <path>] [-Wl,<arg>[,<arg>...]] [-- <lld-arg>...]\n\nReads wat from files, or stdin when no input file or `-` is given. By default, inputs are compiled to temporary objects and linked with `lld`; linked output defaults to `a.out`. Use `-c` to write one relocatable wasm object instead.".to_owned()
}
