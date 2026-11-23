//! the `mestro ctl` client half: connect, send one wire command,
//! print the answer. Exit code 0 = applied.

use std::io::{BufRead, Write};

pub fn client(cmd: &str) -> i32 {
    let path = super::socket_path();
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&path) else {
        eprintln!("ctl: no running instance at {}", path.display());
        return 1;
    };
    if writeln!(stream, "{cmd}").is_err() {
        eprintln!("ctl: write failed");
        return 1;
    }
    let mut answer = String::new();
    let _ = std::io::BufReader::new(&stream).read_line(&mut answer);
    let answer = answer.trim();
    if answer == super::proto::OK {
        0
    } else {
        eprintln!("{}", answer.strip_prefix("err ").unwrap_or(answer));
        1
    }
}
