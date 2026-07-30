use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::ExitCode;

use rss_worker_protocol::{read_request, write_response};

fn main() -> ExitCode {
    let protocol_stdout = match reserve_protocol_stdout() {
        Ok(stdout) => stdout,
        Err(_) => return ExitCode::FAILURE,
    };
    let stdin = io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let request = match read_request(&mut input) {
        Ok(request) => request,
        Err(_) => return ExitCode::FAILURE,
    };
    let mut trailing = [0_u8; 1];
    match input.read(&mut trailing) {
        Ok(0) => {}
        Ok(_) | Err(_) => return ExitCode::FAILURE,
    }

    let response = rss_execution_worker::dispatch(request);
    if response.validate().is_err() {
        return ExitCode::FAILURE;
    }

    let mut output = BufWriter::new(protocol_stdout);
    match write_response(&mut output, &response) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(unix)]
fn reserve_protocol_stdout() -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};

    io::stdout().flush()?;
    let null = File::options().write(true).open("/dev/null")?;
    // SAFETY: `dup` returns an independently owned descriptor on success.
    let protocol_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if protocol_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors are valid; `dup2` atomically replaces fd 1.
    if unsafe { libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO) } < 0 {
        // SAFETY: `protocol_fd` is owned here and has not been wrapped in `File`.
        unsafe { libc::close(protocol_fd) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the successful `dup` result transfers to this File.
    Ok(unsafe { File::from_raw_fd(protocol_fd) })
}

#[cfg(not(unix))]
fn reserve_protocol_stdout() -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "protocol stdout isolation is not implemented on this platform",
    ))
}
