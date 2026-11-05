//! Compatibility launcher for the former `craftsman-rs` command.

use std::process::Command;

fn main() {
    let target = std::env::current_exe()
        .map(|mut path| {
            path.set_file_name(if cfg!(windows) {
                "craftsman.exe"
            } else {
                "craftsman"
            });
            path
        })
        .unwrap_or_else(|_| "craftsman".into());
    let mut command = Command::new(target);
    command.args(std::env::args_os().skip(1));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("craftsman-rs compatibility launcher could not start craftsman: {error}");
        std::process::exit(127);
    }

    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("craftsman-rs compatibility launcher could not start craftsman: {error}");
                std::process::exit(127);
            }
        }
    }
}
