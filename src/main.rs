//! Command-line interface for parsing annotated wat and linking it with lld.
//!
//! The binary reads wat from files, passes it to `rwat::parse_rwat`, and
//! invokes `lld` to link the temporary objects unless `-c` requests a
//! relocatable wasm object.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

struct Args {
    inputs: Vec<PathBuf>,
    output: Option<PathBuf>,
    compile: bool,
    help: bool,
    lld_args: Vec<OsString>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args_os().skip(1))?;
    if args.help {
        println!("usage: rwat [-c] <wat>... [-o|--output <path>] [-Wl,<arg>[,<arg>...]]");
        return Ok(());
    }

    if args.inputs.is_empty() {
        return Err("no input files".into());
    }

    if args.compile {
        if args.output.is_some() && args.inputs.len() > 1 {
            return Err("cannot specify -o when generating multiple output files".into());
        }

        for input in args.inputs {
            let output = match &args.output {
                Some(path) => path,
                None => &PathBuf::from(input.with_extension("o").file_name().unwrap()),
            };
            compile(&input, output)?;
        }

        return Ok(());
    }

    let mut temp_objects = TempObjects { paths: vec![] };
    for (index, input) in args.inputs.into_iter().enumerate() {
        let output = PathBuf::from(format!(".rwat-{}-{index}.o", std::process::id()));
        compile(&input, &output)?;
        temp_objects.paths.push(output);
    }

    Command::new("lld")
        .arg("-flavor")
        .arg("wasm")
        .args(args.lld_args)
        .args(&temp_objects.paths)
        .arg("-o")
        .arg(args.output.unwrap_or_else(|| PathBuf::from("a.out")))
        .status()
        .map_err(|err| format!("failed to run `lld -flavor wasm`: {err}"))?;

    Ok(())
}

fn compile(input: &Path, output: &Path) -> Result<(), String> {
    let wat = fs::read_to_string(input).map_err(|err| format!("`{}`: {err}", input.display()))?;
    let wasm = rwat::parse_rwat(&wat).map_err(|err| err.to_string())?;
    fs::write(output, wasm).map_err(|err| format!("`{}`: {err}", output.display()))?;
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut compile = false;
    let mut help = false;
    let mut lld_args = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        let arg_lossy = arg.to_string_lossy();
        match arg_lossy.as_ref() {
            "-h" | "--help" => help = true,
            "-c" => compile = true,
            "-o" | "--output" => {
                let Some(path) = iter.next() else {
                    return Err(format!("missing value for `{}`", arg_lossy));
                };
                output = Some(PathBuf::from(path));
            }
            "-" => {
                return Err("stdin input is not supported; pass a wat file path".into());
            }
            _ if arg_lossy.starts_with("-Wl,") => {
                let rest = &arg_lossy["-Wl,".len()..];
                lld_args.extend(
                    rest.split(',')
                        .filter(|arg| !arg.is_empty())
                        .map(OsString::from),
                );
            }
            _ if arg_lossy.starts_with('-') => {
                return Err(format!("unknown option `{}`", arg_lossy));
            }
            _ => inputs.push(PathBuf::from(arg)),
        }
    }

    Ok(Args {
        inputs,
        output,
        compile,
        help,
        lld_args,
    })
}

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
