use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let code = gitsync::cli::run(gitsync::version::version(), &args);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
